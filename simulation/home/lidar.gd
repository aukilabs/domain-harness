extends Node3D
## Ideal instantaneous first-return sensor; no access to the map/mesh export.
signal scan_ready(scan: Dictionary)
const COLUMNS := 121
const ROWS := 16
const MAX_RANGE := 10.0
const MIN_RANGE := 0.1
const MOUNT := preload("res://robot_geometry.gd").LIDAR_MOUNT
var robot: CharacterBody3D
var request_scan := false
var show_hits := true
var accumulator := 0.0
var hit_count := 0
var scan_count := 0
var points := MeshInstance3D.new()
var latest: Dictionary = {}
var last_sent_timestamp := 0

func setup(body: CharacterBody3D) -> void:
	robot = body
	points.layers = 4 # Debug returns never appear in the RGB sensor.
	var mat := StandardMaterial3D.new()
	mat.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	mat.albedo_color = Color("40f5bd")
	mat.use_point_size = true
	mat.point_size = 4
	points.material_override = mat
	add_child(points)

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo and event.keycode == KEY_L:
		show_hits = not show_hits
		points.visible = show_hits

func _physics_process(delta: float) -> void:
	accumulator += delta
	if (show_hits and accumulator >= 0.1) or request_scan:
		# A remote request can reuse a just-produced local scan rather than
		# doubling raycasts. Time never resets when the robot is reset.
		if latest.is_empty() or accumulator >= 0.1:
			latest = sample()
			accumulator = 0
		if request_scan and latest.timestamp_ns > last_sent_timestamp:
			request_scan = false
			last_sent_timestamp = latest.timestamp_ns
			scan_ready.emit(latest)

func sample() -> Dictionary:
	var timestamp_ns := Time.get_ticks_usec()*1000
	var sensor := robot.global_transform * Transform3D(Basis.IDENTITY, MOUNT)
	var ranges: Array = []
	var hits := PackedVector3Array()
	var space := get_world_3d().direct_space_state
	for row in ROWS:
		var elevation := deg_to_rad(-15.0 + row * 2.0)
		for column in COLUMNS:
			var yaw := deg_to_rad(-60.0 + column)
			var local_direction := Vector3(sin(yaw)*cos(elevation), sin(elevation), -cos(yaw)*cos(elevation))
			var end := sensor.origin + sensor.basis * local_direction * MAX_RANGE
			var query := PhysicsRayQueryParameters3D.create(sensor.origin, end, 1, [robot.get_rid()])
			var hit := space.intersect_ray(query)
			if hit.is_empty():
				ranges.append(null)
			else:
				var distance: float = sensor.origin.distance_to(hit.position)
				if distance >= MIN_RANGE and distance <= MAX_RANGE:
					ranges.append(distance)
					hits.append(hit.position)
				else:
					ranges.append(null)
	hit_count = hits.size()
	scan_count += 1
	var mesh := ImmediateMesh.new()
	if show_hits and not hits.is_empty():
		mesh.surface_begin(Mesh.PRIMITIVE_POINTS)
		for hit in hits:
			mesh.surface_add_vertex(hit)
		mesh.surface_end()
	points.mesh = mesh
	var q := sensor.basis.get_rotation_quaternion()
	return {"timestamp_ns":timestamp_ns, "ranges_m":ranges,
		"world_position":[sensor.origin.x,sensor.origin.y,sensor.origin.z],
		"world_rotation_xyzw":[q.x,q.y,q.z,q.w]}

extends Node3D
## Metric world. X = east, Y = up, -Z = north.
const ROBOT_GEOMETRY = preload("res://robot_geometry.gd")
const MAX_PUSH_FORCE := 60.0 # Newtons; bounded traction, independent of physics tick rate.

const WALL = Color("e5dfd2")
const WOOD = Color("ab7954")
const DARK = Color("263c43")
const TEAL = Color("408e87")
var robot: CharacterBody3D
var eye: Camera3D
var overview: Camera3D
var status: Label
var pitch := 0.0
var boxes: Array[RigidBody3D] = []
var box_starts: Array[Vector3] = []
var room_labels: Array[Label3D] = []
var camera_bridge: Node
var lidar: Node3D
var testing_wall_contact := false

func material(color: Color) -> StandardMaterial3D:
	var m := StandardMaterial3D.new()
	m.albedo_color = color
	m.roughness = 0.85
	return m

func block(label: String, size: Vector3, at: Vector3, color: Color, solid := true, parent: Node3D = null) -> MeshInstance3D:
	var mesh := MeshInstance3D.new()
	mesh.name = label
	var shape := BoxMesh.new()
	shape.size = size
	mesh.mesh = shape
	mesh.material_override = material(color)
	if parent == null:
		parent = self
	parent.add_child(mesh)
	mesh.position = at
	if solid:
		var body := StaticBody3D.new()
		mesh.add_child(body)
		var collision := CollisionShape3D.new()
		var box := BoxShape3D.new()
		box.size = size
		collision.shape = box
		body.add_child(collision)
	return mesh

func wall_x(x1: float, x2: float, z: float) -> void:
	block("Wall", Vector3(x2-x1, 2.7, 0.16), Vector3((x1+x2)/2, 1.35, z), WALL)
	block("Skirting", Vector3(x2-x1, 0.12, 0.19), Vector3((x1+x2)/2, 0.06, z), Color.WHITE, false)

func wall_z(z1: float, z2: float, x: float) -> void:
	block("Wall", Vector3(0.16, 2.7, z2-z1), Vector3(x, 1.35, (z1+z2)/2), WALL)

func doorway(x1: float, x2: float, z: float, center: float) -> void:
	wall_x(x1, center-0.6, z)
	wall_x(center+0.6, x2, z)
	block("Door lintel", Vector3(1.2, 0.5, 0.16), Vector3(center, 2.45, z), WALL)
	for x in [center-0.62, center+0.62]:
		block("Door trim", Vector3(0.08, 2.2, 0.22), Vector3(x, 1.1, z), WOOD, false)

func floor_area(title: String, x: float, z: float, w: float, d: float, color: Color) -> void:
	block(title+" floor", Vector3(w, 0.1, d), Vector3(x, -0.05, z), color)
	var label := Label3D.new()
	label.text = title.to_upper()
	label.font_size = 48
	label.pixel_size = 0.006
	label.modulate = Color("253c43")
	label.outline_size = 0
	label.no_depth_test = true
	label.layers = 2 # Overview labels must never enter the sensor camera.
	label.rotation_degrees.x = -90
	label.position = Vector3(x, 0.025, z+d/2-0.5)
	add_child(label)
	room_labels.append(label)

func bed(x: float, z: float, color: Color, width := 1.6) -> void:
	block("Bed frame", Vector3(width+0.12, 0.34, 2.2), Vector3(x, 0.25, z), WOOD)
	block("Mattress", Vector3(width, 0.22, 2.05), Vector3(x, 0.53, z), Color("f4f1e8"))
	block("Duvet", Vector3(width+0.02, 0.09, 1.4), Vector3(x, 0.68, z+0.3), color, false)
	block("Headboard", Vector3(width+0.16, 1, 0.12), Vector3(x, 0.65, z-1.12), WOOD)
	block("Pillow", Vector3(width*0.75, 0.12, 0.4), Vector3(x, 0.7, z-0.7), Color.WHITE, false)
	block("Nightstand", Vector3(0.5, 0.55, 0.5), Vector3(x-width/2-0.5, 0.275, z-0.7), WOOD)

func table(x: float, z: float, w: float, d: float) -> void:
	block("Table top", Vector3(w, 0.12, d), Vector3(x, 0.78, z), WOOD)
	for dx in [-w/2+0.12, w/2-0.12]:
		for dz in [-d/2+0.12, d/2-0.12]:
			block("Table leg", Vector3(0.1, 0.72, 0.1), Vector3(x+dx, 0.36, z+dz), DARK)

func bathroom(x: float, z: float) -> void:
	block("Shower tray", Vector3(1.2, 0.12, 1.2), Vector3(x+0.65, 0.06, z-0.7), Color("f5f4ee"))
	block("Shower wall", Vector3(0.06, 1.9, 1.2), Vector3(x+0.05, 0.95, z-0.7), Color("91b8bd"))
	block("Vanity", Vector3(0.8, 0.8, 0.5), Vector3(x-0.7, 0.4, z-0.9), TEAL)
	block("Basin", Vector3(0.82, 0.12, 0.52), Vector3(x-0.7, 0.86, z-0.9), Color.WHITE)
	block("Toilet base", Vector3(0.42, 0.4, 0.65), Vector3(x+0.65, 0.2, z+0.7), Color.WHITE)
	block("Toilet tank", Vector3(0.45, 0.72, 0.2), Vector3(x+0.65, 0.36, z+1), Color.WHITE)

func _ready() -> void:
	var environment := WorldEnvironment.new()
	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color("adc4ce")
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color("fff1dd")
	env.ambient_light_energy = 0.4
	env.tonemap_mode = Environment.TONE_MAPPER_LINEAR
	environment.environment = env
	add_child(environment)
	var sun := DirectionalLight3D.new()
	sun.rotation_degrees = Vector3(-65, -30, 0)
	sun.light_energy = 0.25
	sun.shadow_enabled = true
	add_child(sun)
	block("Foundation", Vector3(16.5, 0.25, 12.5), Vector3(0, -0.24, 0), DARK)
	floor_area("Bedroom 2", -5.5, -3, 5, 6, Color("c9b59b"))
	floor_area("Bedroom 3", -0.5, -3, 5, 6, Color("d7c4aa"))
	floor_area("Main bedroom", 5, -3, 6, 6, Color("c5b49e"))
	floor_area("Ensuite", 6.5, -4.5, 2.84, 2.84, Color("a7c0ba"))
	# Raise the ensuite finish slightly above the bedroom substrate.
	get_node("Ensuite floor").position.y += 0.006
	floor_area("Hallway", 0, 1, 16, 2, Color("e0c9a8"))
	floor_area("Living room", -4.5, 4, 7, 4, Color("d8bd98"))
	floor_area("Kitchen / dining", 2, 4, 6, 4, Color("d6d5c4"))
	floor_area("Bathroom", 6.5, 4, 3, 4, Color("b2c8c6"))
	wall_x(-8, 8, -6)
	wall_x(-8, 8, 6)
	wall_z(-6, 6, -8)
	wall_z(-6, 6, 8)
	wall_z(-6, 0, -3)
	wall_z(-6, 0, 2)
	doorway(-8, -3, 0, -5.5)
	doorway(-3, 2, 0, -0.5)
	doorway(2, 8, 0, 3.5)
	doorway(-8, -1, 2, -4)
	doorway(-1, 5, 2, 2)
	doorway(5, 8, 2, 6.5)
	wall_z(2, 6, 5)
	wall_z(-6, -3, 5)
	doorway(5, 8, -3, 6.5)
	bed(-6.1, -4.3, Color("79999c"))
	bed(-0.5, -4.3, Color("cb976f"), 1.2)
	bed(3.45, -4.3, Color("8b9b79"))
	for x in [-7.3, -2.3, 2.7]:
		block("Wardrobe", Vector3(0.65, 2.1, 1.6), Vector3(x, 1.05, -1.5), Color("b8906d"))
	table(0.75, -1.5, 1.5, 0.65)
	bathroom(6.5, -4.5)
	bathroom(6.5, 4.3)
	block("Living rug", Vector3(3.8, 0.02, 2.5), Vector3(-4.7, 0.015, 4.2), Color("809994"), false)
	block("Sofa seat", Vector3(2.8, 0.45, 0.85), Vector3(-5, 0.35, 5.2), TEAL)
	block("Sofa back", Vector3(2.8, 0.85, 0.2), Vector3(-5, 0.65, 5.65), TEAL)
	for x in [-6.45, -3.55]:
		block("Sofa arm", Vector3(0.2, 0.7, 1), Vector3(x, 0.5, 5.2), TEAL)
	block("Coffee table", Vector3(1.4, 0.4, 0.65), Vector3(-5, 0.2, 3.8), WOOD)
	block("Media console", Vector3(0.45, 0.55, 2), Vector3(-7.55, 0.275, 3.8), WOOD)
	block("TV", Vector3(0.08, 0.8, 1.45), Vector3(-7.55, 1.05, 3.8), DARK)
	for x in [0, 1, 2, 3, 4]:
		block("Kitchen cabinet", Vector3(0.96, 0.88, 0.65), Vector3(x, 0.44, 5.5), TEAL)
		block("Countertop", Vector3(1, 0.07, 0.72), Vector3(x, 0.915, 5.5), Color("f1ebdc"))
	block("Fridge", Vector3(0.8, 1.9, 0.75), Vector3(4.4, 0.95, 3.2), Color("b8c5c5"))
	block("Hob", Vector3(0.65, 0.02, 0.5), Vector3(1, 0.965, 5.5), DARK, false)
	block("Sink", Vector3(0.6, 0.02, 0.45), Vector3(3, 0.965, 5.5), Color("819a9f"), false)
	table(1.3, 3.6, 1.6, 0.85)
	for x in [0.3, 2.3]:
		block("Dining stool", Vector3(0.4, 0.45, 0.4), Vector3(x, 0.225, 3.6), DARK)
	for p in [Vector3(-2, 0.3, 4.6), Vector3(1, 0.3, 1.1)]:
		var box := RigidBody3D.new()
		box.name = "Movable box"
		box.mass = 2
		add_child(box)
		box.position = p
		block("Cardboard", Vector3(0.5, 0.5, 0.5), Vector3.ZERO, Color("c69a66"), false, box)
		var collision := CollisionShape3D.new()
		var shape := BoxShape3D.new()
		shape.size = Vector3(0.5, 0.5, 0.5)
		collision.shape = shape
		box.add_child(collision)
		boxes.append(box)
		box_starts.append(p)
	make_robot()
	lidar = load("res://lidar.gd").new()
	add_child(lidar)
	lidar.setup(robot)
	if "--sdk-peer" in OS.get_cmdline_user_args():
		camera_bridge = load("res://camera_bridge.gd").new()
		add_child(camera_bridge)
		camera_bridge.setup(robot, lidar)
	make_ui()
	if "--smoke-test" in OS.get_cmdline_user_args():
		call_deferred("smoke_test")
	if "--preview" in OS.get_cmdline_user_args():
		call_deferred("save_preview")

func make_robot() -> void:
	robot = CharacterBody3D.new()
	robot.name = "Robot"
	add_child(robot)
	var collider := CollisionShape3D.new()
	# A capsule narrows at its ends; use a full-height cylinder to protect
	# the high camera as well as the forward LiDAR, even while turning.
	var envelope := CylinderShape3D.new()
	envelope.radius = ROBOT_GEOMETRY.COLLISION_RADIUS
	envelope.height = ROBOT_GEOMETRY.COLLISION_HEIGHT
	collider.shape = envelope
	collider.position.y = ROBOT_GEOMETRY.COLLISION_HEIGHT / 2
	robot.add_child(collider)
	block("Robot body", Vector3(0.42, 0.65, 0.4), Vector3(0, 0.4, 0), TEAL, false, robot)
	block("Robot face", Vector3(0.3, 0.15, 0.04), Vector3(0, 0.65, -0.22), DARK, false, robot)
	eye = Camera3D.new()
	eye.position = ROBOT_GEOMETRY.CAMERA_MOUNT
	eye.near = ROBOT_GEOMETRY.CAMERA_NEAR
	eye.fov = ROBOT_GEOMETRY.CAMERA_FOV
	robot.add_child(eye)
	overview = Camera3D.new()
	add_child(overview)
	overview.position = Vector3(0, 26, 7)
	overview.look_at(Vector3.ZERO)
	overview.projection = Camera3D.PROJECTION_ORTHOGONAL
	overview.size = 21
	overview.h_offset = -2.7
	eye.make_current()
	reset_world()

func make_ui() -> void:
	var canvas := CanvasLayer.new()
	add_child(canvas)
	var panel := PanelContainer.new()
	panel.position = Vector2(24, 20)
	var style := StyleBoxFlat.new()
	style.bg_color = Color(0.035, 0.07, 0.09, 0.92)
	style.content_margin_left = 20
	style.content_margin_right = 20
	style.content_margin_top = 14
	style.content_margin_bottom = 14
	style.corner_radius_top_left = 10
	style.corner_radius_bottom_right = 10
	panel.add_theme_stylebox_override("panel", style)
	canvas.add_child(panel)
	var layout := VBoxContainer.new()
	layout.add_theme_constant_override("separation", 7)
	panel.add_child(layout)
	var title := Label.new()
	title.text = "HOME / ROBOT PLAYGROUND"
	title.add_theme_font_size_override("font_size", 22)
	layout.add_child(title)
	status = Label.new()
	status.add_theme_color_override("font_color", Color("80d7c6"))
	layout.add_child(status)
	var help := Label.new()
	help.text = "W/S drive · A/D turn · Shift slow\nTab switch view · Click for mouse look · Esc release\nL toggle LiDAR returns · R reset robot + boxes"
	help.add_theme_font_size_override("font_size", 14)
	layout.add_child(help)
	var note := Label.new()
	note.text = "192 m²  /  3 bedrooms  /  2 bathrooms  /  120° LiDAR"
	note.add_theme_font_size_override("font_size", 12)
	layout.add_child(note)

func reset_world() -> void:
	robot.position = Vector3(-4, 0.05, 1)
	robot.rotation = Vector3(0, -PI/2, 0)
	robot.velocity = Vector3.ZERO
	pitch = 0
	eye.rotation.x = 0
	for i in boxes.size():
		PhysicsServer3D.body_set_state(boxes[i].get_rid(), PhysicsServer3D.BODY_STATE_TRANSFORM, Transform3D(Basis.IDENTITY, box_starts[i]))
		boxes[i].linear_velocity = Vector3.ZERO
		boxes[i].angular_velocity = Vector3.ZERO

func _unhandled_input(event: InputEvent) -> void:
	if event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_TAB:
			if eye.current:
				overview.make_current()
			else:
				eye.make_current()
			Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
		if event.keycode == KEY_ESCAPE:
			Input.mouse_mode = Input.MOUSE_MODE_VISIBLE
		if event.keycode == KEY_R:
			reset_world()
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT and eye.current:
		Input.mouse_mode = Input.MOUSE_MODE_CAPTURED
	if event is InputEventMouseMotion and Input.mouse_mode == Input.MOUSE_MODE_CAPTURED:
		robot.rotate_y(-event.relative.x * 0.003)
		pitch = clampf(pitch-event.relative.y*0.003, -1.1, 1.1)
		eye.rotation.x = pitch

func _physics_process(delta: float) -> void:
	if testing_wall_contact:
		return
	var turn := float(Input.is_physical_key_pressed(KEY_A))-float(Input.is_physical_key_pressed(KEY_D))
	var drive := float(Input.is_physical_key_pressed(KEY_W))-float(Input.is_physical_key_pressed(KEY_S))
	robot.rotate_y(turn*1.8*delta)
	var speed := 0.6 if Input.is_physical_key_pressed(KEY_SHIFT) else 1.8
	var direction := -robot.basis.z*drive*speed
	robot.velocity.x = direction.x
	robot.velocity.z = direction.z
	robot.velocity.y -= 9.8*delta
	robot.move_and_slide()
	push_bodies(direction, delta)
	for label in room_labels:
		label.visible = overview.current
	status.text = "%s  /  X %.1f m  Z %.1f m" % ["OVERVIEW" if overview.current else "ROBOT EYE", robot.position.x, robot.position.z]
	if camera_bridge:
		status.text += "\n" + camera_bridge.status_text
	status.text += "\nLiDAR · %d / 1936 returns · %s" % [lidar.hit_count, "L to hide" if lidar.show_hits else "L to inspect"]

func push_bodies(drive_velocity: Vector3, delta: float) -> void:
	var pushed: Array[RID] = []
	for i in robot.get_slide_collision_count():
		var hit := robot.get_slide_collision(i)
		var body := hit.get_collider() as RigidBody3D
		if body == null or pushed.has(body.get_rid()) or abs(hit.get_normal().y) > 0.5:
			continue
		var normal := -hit.get_normal()
		normal.y = 0
		normal = normal.normalized()
		var target_speed := drive_velocity.dot(normal)
		if target_speed <= 0:
			continue # Standing against a box, turning, or reversing away cannot push it.
		var closing_speed := maxf(0, target_speed-body.linear_velocity.dot(normal))
		var impulse := minf(body.mass*closing_speed, MAX_PUSH_FORCE*delta)
		body.apply_central_impulse(normal*impulse)
		pushed.append(body.get_rid())

func smoke_test() -> void:
	await get_tree().physics_frame
	await get_tree().physics_frame
	assert(eye.current, "First-person camera should be the default")
	var toggle := InputEventKey.new()
	toggle.keycode = KEY_TAB
	toggle.pressed = true
	_unhandled_input(toggle)
	assert(overview.current, "Tab should select overview")
	_unhandled_input(toggle)
	assert(eye.current, "Tab should return to first person")
	assert(room_labels.size() == 8, "Expected all eight room/area labels")
	assert(boxes.size() == 2)
	# Robot fits through every 1.2 m doorway, but walls block it.
	for x in [-5.5, -0.5, 3.5]:
		robot.position = Vector3(x, 0.02, 0.8)
		assert(not robot.test_move(robot.global_transform, Vector3(0, 0, -1.6)), "Blocked bedroom doorway")
	for x in [-4.0, 2.0, 6.5]:
		robot.position = Vector3(x, 0.02, 1.2)
		assert(not robot.test_move(robot.global_transform, Vector3(0, 0, 1.2)), "Blocked south doorway at x=%s" % x)
	robot.position = Vector3(6.5, 0.02, -2.2)
	assert(not robot.test_move(robot.global_transform, Vector3(0, 0, -1.6)), "Blocked ensuite doorway")
	robot.position = Vector3(-4, 0.02, 0.8)
	assert(robot.test_move(robot.global_transform, Vector3(0, 0, -1.6)), "Missing partition collision")
	reset_world()
	var start := robot.position
	var forward := InputEventKey.new()
	forward.physical_keycode = KEY_W
	forward.pressed = true
	Input.parse_input_event(forward)
	for i in 30:
		await get_tree().physics_frame
	forward.pressed = false
	Input.parse_input_event(forward)
	assert(robot.position.distance_to(start) > 0.5, "Drive input did not move robot")
	reset_world()
	assert(robot.position.is_equal_approx(start), "Reset did not restore spawn")
	await test_box_pushing()
	# Isolated geometry: a near wall must occlude a second wall, and neither
	# may appear when the sensor faces away. Keep both outside the home.
	robot.position = Vector3(40, 0.02, 40)
	robot.rotation = Vector3.ZERO
	var near_wall := block("LiDAR test near", Vector3(20, 6, 0.1), Vector3(40, 2, 37), WALL)
	var far_wall := block("LiDAR test far", Vector3(20, 6, 0.1), Vector3(40, 2, 35), WALL)
	await get_tree().physics_frame
	await get_tree().physics_frame
	var scan: Dictionary = lidar.sample()
	assert(scan.ranges_m.size() == 1936)
	var central: float = scan.ranges_m[7*121+60]
	var forward_distance: float = scan.world_position[2]-37.05
	assert(abs(central-forward_distance/cos(deg_to_rad(1.0))) < 0.02, "LiDAR must hit the near wall")
	assert(abs(float(scan.ranges_m[7*121])-forward_distance/(cos(deg_to_rad(60.0))*cos(deg_to_rad(1.0)))) < 0.03, "LiDAR edge must be at -60 degrees")
	assert(abs(float(scan.ranges_m[7*121+120])-float(scan.ranges_m[7*121])) < 0.03, "LiDAR sweep must be symmetric")
	robot.rotation.y = PI
	scan = lidar.sample()
	assert(scan.ranges_m.all(func(r): return r == null), "LiDAR must not see behind the robot")
	near_wall.free()
	far_wall.free()
	await test_sensor_wall_clearance()
	reset_world()
	print("LIDAR_SMOKE_OK: 1936 beams, 120 degree sweep, first-hit occlusion, forward-only")
	print("HOME_SMOKE_OK: rooms, boxes, doorway clearance, wall collision, reset")
	get_tree().quit()

func test_box_pushing() -> void:
	reset_world()
	# Drive into the actual hallway box, along the open corridor.
	robot.position = Vector3(-0.1, 0.02, 1.1)
	robot.rotation = Vector3(0, -PI/2, 0)
	for i in 30:
		await get_tree().physics_frame
	var box := boxes[1]
	var start := box.position
	var forward := InputEventKey.new()
	forward.physical_keycode = KEY_W
	forward.pressed = true
	Input.parse_input_event(forward)
	for i in 90:
		await get_tree().physics_frame
	forward.pressed = false
	Input.parse_input_event(forward)
	print("BOX_PUSH_DISTANCE: ", box.position.x-start.x)
	assert(box.position.x-start.x > 0.4, "Driving into the hallway box must push it")
	assert(abs(box.position.z-start.z) < 0.25, "Push should follow the driving direction")
	assert(abs(box.position.y-start.y) < 0.1, "Push must not launch the box vertically")
	for i in 90:
		await get_tree().physics_frame
	assert(box.linear_velocity.length() < 0.1, "Box must settle when driving stops")
	print("BOX_PUSH_OK: hallway box moves under drive, stays grounded, settles on release")
	reset_world()

func test_sensor_wall_clearance() -> void:
	testing_wall_contact = true
	var wall := block("Sensor clearance wall", Vector3(20, 6, 0.1), Vector3(40, 2, 37), WALL)
	var side := block("Sensor clearance corner", Vector3(0.1, 6, 10), Vector3(42, 2, 40), WALL)
	await get_tree().physics_frame
	await get_tree().physics_frame
	# Use actual swept collision, not a manually chosen safe final position.
	for yaw in [-60.0, 0.0, 60.0]:
		robot.position = Vector3(40, 0.02, 40)
		robot.rotation = Vector3(0, deg_to_rad(yaw), 0)
		assert(robot.move_and_collide(Vector3(0, 0, -5)) != null, "Robot must stop at wall")
		for angle in [-1.1, 0.0, 1.1]:
			eye.rotation.x = angle
			assert_camera_clearance(37.05, 41.95)
		var origin: Vector3 = robot.global_transform * ROBOT_GEOMETRY.LIDAR_MOUNT
		assert(origin.z > 37.05, "LiDAR origin penetrated wall")
		if yaw == 0.0:
			var scan: Dictionary = lidar.sample()
			var distance: float = scan.ranges_m[7*121+60]
			assert(distance >= 0.1 and distance < 0.2, "Contact scan must return the near wall, not see beyond it")
	# Push into an inside corner, then rotate in place. A circular footprint
	# must keep protecting the sensors throughout the rotation.
	robot.position = Vector3(40, 0.02, 40)
	robot.move_and_collide(Vector3(4, 0, 0))
	robot.move_and_collide(Vector3(0, 0, -5))
	for yaw in range(0, 360, 15):
		robot.rotation.y = deg_to_rad(yaw)
		for angle in [-1.1, 0.0, 1.1]:
			eye.rotation.x = angle
			assert_camera_clearance(37.05, 41.95)
		var origin: Vector3 = robot.global_transform * ROBOT_GEOMETRY.LIDAR_MOUNT
		assert(origin.z > 37.05 and origin.x < 41.95, "LiDAR crossed the corner while turning")
	wall.free()
	side.free()
	# A shelf at camera height must block the robot even though its lower
	# body fits below it. The former 0.9 m capsule failed this case.
	var shelf := block("Camera-height obstruction", Vector3(20, 0.2, 0.1), Vector3(40, 1.15, 37), WALL)
	robot.position = Vector3(40, 0.02, 40)
	robot.rotation = Vector3.ZERO
	await get_tree().physics_frame
	await get_tree().physics_frame
	assert(robot.move_and_collide(Vector3(0, 0, -5)) != null, "Camera-height obstacles must collide")
	shelf.free()
	testing_wall_contact = false
	print("SENSOR_CLEARANCE_OK: wall contact, near-plane corners, pitch, 360-degree corner turns, high obstacles")

func assert_camera_clearance(front_z: float, side_x: float) -> void:
	var size := get_viewport().get_visible_rect().size
	for pixel in [Vector2.ZERO, Vector2(size.x, 0), size, Vector2(0, size.y)]:
		var corner := eye.project_position(pixel, eye.near)
		assert(corner.z > front_z and corner.x < side_x, "Camera near plane crossed wall")
	# The fixed RGB sensor shares the same mount/near/FOV, with a 4:3
	# viewport. Its near rectangle is inside the tested wider player view.

func save_preview() -> void:
	overview.make_current()
	for i in 12:
		await get_tree().process_frame
	await RenderingServer.frame_post_draw
	get_viewport().get_texture().get_image().save_png("/tmp/home-playground-overview.png")
	eye.current = true
	await get_tree().process_frame
	await RenderingServer.frame_post_draw
	get_viewport().get_texture().get_image().save_png("/tmp/home-playground-eye.png")
	get_tree().quit()

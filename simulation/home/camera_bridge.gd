extends Node
const ROBOT_GEOMETRY = preload("res://robot_geometry.gd")
## Local-only camera adapter. The Rust process owns all SDK authentication.
## One request in flight; viewport rendering/encoding happens only on demand.

var robot: Node3D
var viewport: SubViewport
var camera: Camera3D
var lidar: Node3D
var stream := StreamPeerTCP.new()
var busy := false
var retry_at := 0
var captured := 0
var status_text := "Camera peer: connecting"

func setup(body: Node3D, rangefinder: Node3D) -> void:
	robot = body
	lidar = rangefinder
	viewport = SubViewport.new()
	viewport.size = Vector2i(640, 480)
	viewport.world_3d = body.get_world_3d()
	viewport.render_target_update_mode = SubViewport.UPDATE_DISABLED
	add_child(viewport)
	camera = Camera3D.new()
	camera.fov = ROBOT_GEOMETRY.CAMERA_FOV
	camera.near = ROBOT_GEOMETRY.CAMERA_NEAR
	camera.cull_mask = 1
	viewport.add_child(camera)
	camera.make_current()
	stream.big_endian = false

func _process(_delta: float) -> void:
	stream.poll()
	var state := stream.get_status()
	if state == StreamPeerTCP.STATUS_NONE or state == StreamPeerTCP.STATUS_ERROR:
		status_text = "Camera peer: disconnected (no frames sent)"
		if Time.get_ticks_msec() >= retry_at:
			stream.disconnect_from_host()
			stream.connect_to_host("127.0.0.1", 4331)
			retry_at = Time.get_ticks_msec() + 1000
		return
	if state != StreamPeerTCP.STATUS_CONNECTED or busy:
		return
	status_text = "Camera peer connected · %d frames captured on demand" % captured
	if stream.get_available_bytes() > 0:
		match stream.get_u8():
			1: capture_frame()
			2: capture_lidar()
			_: stream.disconnect_from_host()

func capture_lidar() -> void:
	busy = true
	lidar.request_scan = true
	var scan: Dictionary = await lidar.scan_ready
	var data := JSON.stringify(scan).to_utf8_buffer()
	if stream.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		stream.put_u32(data.size())
		stream.put_data(data)
	busy = false

func capture_frame() -> void:
	busy = true
	# Dedicated body-mounted camera, independent of eye pitch and overview.
	camera.global_transform = robot.global_transform * Transform3D(Basis.IDENTITY, ROBOT_GEOMETRY.CAMERA_MOUNT)
	# Same-host UTC capture reading; the Rust sidecar explicitly maps it into
	# its SDK session clock. Never timestamp after rendering or encoding.
	var timestamp_ns := int(Time.get_unix_time_from_system() * 1000000000.0)
	viewport.render_target_update_mode = SubViewport.UPDATE_ONCE
	await RenderingServer.frame_post_draw
	var jpeg := viewport.get_texture().get_image().save_jpg_to_buffer(0.7)
	if stream.get_status() == StreamPeerTCP.STATUS_CONNECTED:
		stream.put_u32(jpeg.size())
		stream.put_u64(timestamp_ns)
		stream.put_data(jpeg)
		captured += 1
	busy = false

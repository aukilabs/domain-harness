# Scene geometry tools

Home binds queries to the selected scene and the camera captured when Send is
pressed. Coordinates are XYZ meters, Y up; ground is Y=0. Queries never write
scene state. Volume rotation/translation and point offsets use `auki-geometry`
spatial-transform composition.

- `get_volume_corners(volume_id)` returns exact rotated ground corner positions,
  stable IDs and viewer-relative near/far/left/right labels. Side ties choose
  the farther corner; remaining exact ties are deterministic.
- `point_inside_volume(volume_id, corner, distance_m, mode)` computes an inset.
  `toward_center` means diagonal distance; `from_walls` means that distance from
  each adjoining wall. Insets beyond the center/half-size are rejected.
- `offset_point(position, direction, distance_m, frame, ground)` computes a
  viewer/scene-relative offset. `ground=true` projects onto the horizontal plane
  and returns Y=0. Toward-viewer offsets stop at the projected camera position.

For existing notes: get_note, geometry query, then move_note with original ID
and expected_position. Preserve name/description; never create/delete a replacement.

“One meter inside the farthest corner” uses corner=far, mode=toward_center.
“One meter closer to me on the ground” uses toward_viewer, viewer, ground=true.

Machine schemas live in `config/scene-geometry-tools.json`. Home loads only
relevant schemas; other tools can be discovered with enable_scene_tools. This
Markdown is for maintainers, not injected into every request. Documentation on
disk alone does not give Qwen callable tools; compact schemas are still needed.

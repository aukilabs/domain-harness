# Home's Qwen tool harness

Inventory checked against the `harness` branch on 2026-10-14. There are **36 distinct Qwen function tools**: 12 read-only tools, 19 scene/metadata mutations, four browser-view actions, and one tool-discovery helper.

This is the union of tools Home can give Qwen, not a claim that every tool is included in every model request. It is also distinct from the SDK Catalog: a peer advertising an Operable does not automatically make it a Qwen tool.

## Conventions and scope

- Scene operations are bound by Home to the selected scene. Qwen cannot choose a filesystem path or substitute a coordinate frame.
- Positions, centers, and sizes are XYZ arrays in meters, with Y up. The reference floor is Y=0; it is not a measured floor surface. Yaw is in radians.
- Arguments marked `?` below are optional. All other listed arguments are required. An empty description is allowed where `description` is required.
- `expected_*` arguments are the exact values from a current read, used to reject conflicting edits. Existing-volume edits require a fresh `get_semantic_volume` call; a successful volume edit consumes that read, so a dependent edit must read again.
- Peer names and aliases must resolve to exact peer IDs. Camera and frame selections use advertised Product IDs and registered frames, not device-specific guesses.
- Peer sensor reads use the SDK/libp2p path. Browser actions affect this user's Home viewer; they do not move hardware.

## Discovery and current state — 7 tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `enable_scene_tools` | `category`, `operation?` | Immediately enables a whole category, preserving all previously enabled tools. Categories: `notes`, `volumes`, `peers`, `voxels`, `geometry`. Optional exact operation is validated for compatibility but still loads its related group. Does not execute an operation. |
| `inspect_scenegraph` | None | Reads the selected scene's identity, frame, notes, portals/anchors, landmarks, semantic volumes, peer discovery metadata, and aliases. Does not resolve live poses or wait for unrelated peers. Connected status is not a location. |
| `get_peer_pose` | `peer_id` | Resolves only the requested peer through the same registry-backed live-pose resolver as the viewer. Returns position/orientation in the selected scene, arrival freshness, exact capture nanoseconds/clock, Product/Output/sequence and frame/session provenance. Stale, lost, ambiguous, or uncalibrated observations fail; saved labels are not a fallback. |
| `inspect_catalog` | `peer_id?`, `section?`, `offset?`, `limit?` | Without a peer ID, lists peers and aliases. With a peer ID, pages `components` or `products`, including advertised observables and Operables. Page size defaults to 5, maximum 10. Reports cached/unavailable status; does not execute capabilities or prove stream freshness. |
| `inspect_sensor_frames` | `peer_id` | Lists a discovered peer's registry-backed camera and sampled-pose frames, with exact Product IDs. Discovery alone does not open streams or establish valid tracking. |
| `get_note` | `id` | Reads an existing note's current name, description, position, and frame. Live peer labels must not be treated as editable authored notes. |
| `get_semantic_volume` | `id` | Reads a semantic volume in the selected scene, including its current geometry, metadata, and calculated bottom center. |

## Camera feeds and the browser viewpoint — 5 tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `open_camera_pip` | `peer_id`, `product_id?` | Requests one peer camera in the browser's PiP, replacing the current PiP. Product ID may be omitted only when the peer has a single advertised camera; otherwise Qwen should select the best matching advertised Product. Does not open all cameras. |
| `capture_camera_frame` | `peer_id`, `product_id` | Obtains one fresh image for a visual question, with capture provenance. Home runs a separate, tool-free vision pass to describe it. Does not open PiP, require spatial calibration, or move hardware; unavailable capture is an error, not permission to use an old image. |
| `move_camera` | `position`, `look_at` | Moves this browser's virtual 3D map camera to a position aimed at a point in the selected scene. Upright, without roll; position-to-target distance must be 0.01–60 m. Does not move a peer or edit an object. |
| `attach_camera` | `peer_id`, `frame_id`, `product_id?` | Continuously follows a peer sensor frame with the browser's virtual map camera. Requires a unique advertised frame/Product and a valid live transform into the selected Home scene. Pauses on stale/lost tracking. This is a viewpoint, not a video feed. |
| `detach_camera` | None | Stops following a peer sensor and restores manual map-camera control at its current location. |

`capture_camera_frame` is read-only. The other four tools return browser UI actions, not saved scene edits.

## Authored notes — 8 tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `create_note` | `name`, `description`, `position` | Creates a note at an explicit or server-calculated scene position. Home generates its ID. |
| `create_note_relative_to_view` | `name`, `description`, `reference_id`, `reference_kind`, `direction`, `distance_m`, `elevation_y?` | Creates a note offset from a `volume`, `note`, `anchor`, or `peer`. Directions: `left`, `right`, `behind`, `in_front`. Peer references use the peer's current orientation; other references use the captured viewer perspective. An explicit viewer-perspective request overrides peer axes. Preserves reference elevation unless an absolute Y is supplied or ground placement is requested. |
| `create_note_at_volume_corner` | `name`, `description`, `volume_id`, `corner` | Creates a note at the volume's `far` or `near` bottom corner relative to the captured 3D viewpoint, inset up to 10 cm inside the box. Home computes the position; this is not an existing-note move. |
| `move_note` | `id`, `expected_position`, `position` | Moves an authored note, preserving its name and description. |
| `rename_note` | `id`, `expected_name`, `name` | Replaces a note's name, preserving its position and description. |
| `change_note_description` | `id`, `expected_description`, `description` | Replaces the complete description. For an append request, Qwen must include the existing text in the replacement. |
| `delete_note` | `id`, `expected_name` | Deletes one requested authored note. Not a way to remove system-generated live peer labels or implement bulk deletion. |
| `delete_all_notes` | `expected_ids` | Atomically deletes all authored notes in the selected scene after an explicit bulk-delete request. Requires the complete current set of authored note IDs; excludes live peer labels and preserves volumes, voxels, and other scenes. |

## Semantic volumes — 9 tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `create_volume` | `name`, `description?`, `center?`, `size?`, `yaw?` | Creates a semantic box with a generated ID and type `region`. A name-only request defaults to an empty description and a 1 m cube resting on Y=0: center `[0, 0.5, 0]`, size `[1, 1, 1]`, yaw `0`. |
| `move_volume` | `id`, `expected_center`, `center` | Sets the volume center, preserving size, orientation, and metadata. |
| `resize_volume` | `id`, `expected_size`, `size` | Sets XYZ dimensions. Changes to height automatically adjust center Y to preserve the bottom elevation; center X/Z and orientation remain unchanged. |
| `set_volume_length` | `id`, `expected_size`, `axis`, `length` | Sets one dimension. Axis is `x`, `y`, `z`, `longest`, or `longest_horizontal`. Height changes preserve bottom elevation; other dimensions remain unchanged. |
| `place_volume_relative_to_floor` | `id`, `expected_center`, `offset_m` | Sets the volume's bottom to an absolute signed elevation relative to Y=0. Home calculates center Y as offset plus half height; X/Z, size, orientation, and metadata remain unchanged. |
| `rename_volume` | `id`, `expected_name`, `name` | Changes the volume title, preserving other metadata and geometry. |
| `update_volume_description` | `id`, `expected_description`, `description` | Replaces the complete description, preserving geometry and other metadata. Append requests must supply the full existing-plus-new text. |
| `delete_volume` | `id`, `expected_name` | Deletes one specifically requested semantic volume after a fresh read. Does not substitute a note or another volume for an unresolved target. |
| `delete_all_volumes` | `expected_ids` | Atomically deletes all semantic volumes in the selected scene after an explicit bulk-delete request. Requires all current volume IDs; preserves notes, voxels, anchors, and transforms. |

## Read-only spatial calculations — 5 tools

These tools return geometry. They do not create or move notes by themselves; a separate edit tool must consume the result.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `get_volume_corners` | `volume_id` | Returns four rotated ground-plane corners at Y=0, stable corner IDs, and near/far/left/right labels relative to the captured viewer. |
| `point_inside_volume` | `volume_id`, `corner`, `distance_m`, `mode` | Calculates a ground point inward from a corner ID or `far`/`near`/`left`/`right`. Mode `toward_center` measures diagonally toward the center; `from_walls` measures from both adjoining walls. |
| `offset_point` | `position`, `direction`, `distance_m`, `frame`, `ground` | Offsets a point in `viewer` or `scene` axes. Directions: `toward_viewer`, `away_from_viewer`, `left`, `right`, `forward`, `backward`, `up`, `down`. `ground=true` uses a horizontal direction and places the result at Y=0. Toward-viewer movement stops at the viewer rather than passing through it. |
| `point_relative_to_peer` | `peer_id`, `direction`, `distance_m`, `elevation_y?` | Calculates an offset using the peer's current registered orientation, not the viewer's. Directions: `in_front`, `behind`, `left`, `right`. Uses the same generic live-pose resolver as the viewer; missing orientation fails. Optional elevation sets absolute scene Y. |
| `raycast_peer_camera` | `peer_id`, `product_id?`, `pixel?` | Uses a camera's capture-matched calibration and transform chain to raycast against occupied Home voxels or the Y=0 floor, taking the nearest hit within 40 m. `pixel=[x,y]` uses a top-left image origin; omitting it uses the optical axis. A missing hit means no note placement. Product selection must resolve uniquely. |

## Peer metadata and voxel reset — 2 tools

| Tool | Arguments | What it does |
| --- | --- | --- |
| `set_peer_alias` | `peer_id`, `alias` | Saves a Home-local alias, up to 64 characters, for an exact peer ID. An empty alias removes it. Does not change transport identity or the peer's own reported name. |
| `clear_voxels` | None | Clears occupied and free voxel evidence in the selected Home-owned local scene after an explicit clear/reset request. Preserves notes, volumes, anchors, and localization transforms. Does not stop scanning; fresh observations can repopulate the map. Cannot clear another peer's private map. |

## How tools reach Qwen

Home always includes these 12 tools in the selected tool set:

- `inspect_scenegraph`, `get_note`, `get_semantic_volume`, `enable_scene_tools`
- `inspect_catalog`, `inspect_sensor_frames`, `get_peer_pose`
- `open_camera_pip`, `capture_camera_frame`, `move_camera`, `attach_camera`, `detach_camera`

The other 24 are selected as related groups from the latest instruction or discovered through `enable_scene_tools`. A category-only call enables the group immediately; a second operation-selection round is unnecessary. Notes and volumes include their geometry helpers. Loading another category never removes earlier tools. The union is deduplicated and bounded to the 36 known schemas; this is not arbitrary SDK operation execution.

Home validates tool arguments and applies edits, rather than allowing model text to mutate state. Disabled or unknown tools cannot dispatch. Successful scene edits return saved receipts; successful viewpoint/PiP requests return browser actions. Neither ends the loop immediately: Qwen can read the receipt and continue with other requested steps, such as creating a note and then revealing it.

## Execution and failure semantics

- The controller allows at most eight planning rounds, four calls per model response, and eight scene-edit/browser-action attempts per request. Reads may repeat within the planning limit.
- Identical normalized edit/action calls run at most once per request. A repeated call returns its previous outcome, including uncertain failures; it does not retry the write. This is not cross-request idempotency, an atomic transaction, or a rollback mechanism.
- Saved receipts and queued browser actions survive later validation, tool, model, or limit failures. Edit/action replies are grounded in those receipts, not unverified model success prose. Natural-language interpretation can still miss an intended step; `status: finished` means the model loop ended, not proof that every part of the user's request was satisfied.
- `errors` contains unresolved failures; corrected calls for the same logical target move earlier errors to `recovered_errors`. A successful read for peer A does not erase a failure for peer B. Unresolved failures produce `partial` or `incomplete`, not an all-success claim.
- The browser executes returned actions once, in order, checking the selected scene/frame at each step. A browser failure stops the remaining actions while preserving earlier browser receipts, saved scene edits, server errors, and any visual description.
- Browser actions are deferred until the response arrives. View-relative geometry uses the request-start viewpoint, so such tools are rejected after queuing a map-camera move/attach/detach. Make a follow-up request after the viewpoint changes. Explicit scene coordinates and independent peer-pose reads can still be used.
- One camera image can be described per request, using a separate tool-free model pass. Neither pixels nor image-derived prose goes back into the action planner. The description is delivered in `camera_description` and the user-facing reply, with `camera_frame` provenance and no embedded image bytes. This does not enable visual-content-dependent actions.

## Validation for this pass

- `src/chatbot_loop_tests.rs`: scripted inference exercises multi-step completion, receipt retention, duplicate prevention, tool discovery, explicit pose reads, recovered errors, limits, and vision isolation without Ollama or live scenes.
- `tests/peer_locations.rs`: authenticated local SDK peers exercise the real generic pose consumer, including capture provenance, transformation into Home, and stale/lost rejection.
- `tests/chat-browser-actions.test.mjs`: ordered browser actions, scene/frame guards, partial failures, and preserved descriptions/receipts.
- `tests/chatbot_ollama.rs`: opt-in live-model scenarios; these are separate from deterministic controller tests and are not automatically run.

## Capabilities not exposed as Qwen tools

- No generic “invoke any SDK Operable” tool. `inspect_catalog` only reads advertisements.
- No `set_enabled` Qwen tool. The generic Devices UI can toggle supported component publication, but this is a separate control path.
- No robot drive, navigation-goal, waypoint, joint/arm movement, or speech tool in this harness.
- No shell execution, arbitrary file access, or arbitrary HTTP-request tool.
- No dedicated distance-between-objects tool. Qwen can answer using positions returned by current scene/live-pose reads.
- No dedicated rotate-existing-volume or change-semantic-type tool. `create_volume` accepts an initial yaw and uses type `region`.

## Source of truth

Tool selection and orchestration: `src/chatbot.rs`.

Tool definitions: `src/scene_tools.rs`, `src/scene_geometry.rs`, `config/scene-geometry-tools.json`, `src/catalog_chat.rs`, `src/view_camera.rs`, `src/camera_vision.rs`, and `src/peer_locations.rs`.

Execution and live spatial resolution: `src/main.rs` (`chat_message`), `src/peer_locations.rs`, and `src/chat_camera.rs`. See also `docs/qwen-tooling-audit.md` for the earlier behavior audit.

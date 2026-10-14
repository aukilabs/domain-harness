# Navigation as an Operable Component

Status: built in Home (`src/scene_navigation.rs`) and used by the iPhone app, which
keeps its own planner for a Home built before this. "Not built yet" below lists
what is still a proposal.

## Why

A phone can now lead its user to a note: it plans a way across the floor and
draws it in AR. That planner lives inside the iPhone app. Nothing
else can ask for a route, Android and robots would each write their own, and the
phone plans on the copy of the map Home last sent it, not on what Home knows.

Routing should be one thing a peer asks for and another peer answers: an
Operable on a Component, discovered through the SDK Catalog like every other
operation here.

## The Component

- **Owner:** Home, one per scene, as `scene-navigation:<scene id>`. Home holds
  the merged map; a phone plans on the copy it was last sent.
- **Callers:** any authenticated peer of the scene: phones, robots, Home's own
  page. Same admission as `scene-paths` and `home-phone-portals`.
- **Exposure:** `Cluster`, exported on Home's `ComponentProtocolEndpoint`.

## Operable `plan`

Instruction `spatial-harness.route-request/v1`:

| Field | Type | Meaning |
| --- | --- | --- |
| `frame` | string | The scene frame every point below is in. Home refuses any other. |
| `from` | place | Where the route starts. |
| `to` | place | Where it ends. |
| `agent` | optional | `radius_m` and `height_m` of what is walking or driving. Defaults to a person (0.25 m, 1.9 m). |
| `share` | optional | `false` returns the route without saving it. Defaults to saving. |

A place is either `{"point": [x, y, z]}` in `frame`, or `{"note": "<note id>"}`
for a note in the scene. Naming a note lets Home use the note's current
position, so note-to-note routes follow a note that is moved.

Result `spatial-harness.route/v1`:

| Field | Type | Meaning |
| --- | --- | --- |
| `frame` | string | The frame the points are in; equals the request's. |
| `status` | string | `found`, `direct` (nothing known in the way) or `unreachable`. |
| `path` | scene-paths `Stroke` | The route as a path: ID, title, color, width and one segment of points on the floor, both ends included. Absent when unreachable, or when the ends are the same place. |
| `length_m` | number | Length along the route. |
| `map_timestamp_ns`, `clock` | | When Home read the map the route was planned on, and the clock that time is under. |

Rules, from `AGENTS.md`:

- Frames are declared, never inferred. A request for a frame that is not the
  scene's current frame is refused, not relabelled.
- A route is valid for the map it was planned on. `map_timestamp_ns` lets a
  caller tell a fresh plan from a stale one.
- Unknown floor is not free floor. The planner crosses floor the map has not
  seen at a cost, and `direct` means only that nothing known is in the way; it
  never reports unseen floor as clear.
- A route too long to plan (more than 320 cells either way) is refused, not
  answered with a guess.

## A route is a path

A route is a line of points in the scene frame, which is what a Crayon stroke
is. It is a path drawn without a crayon. So the result carries a scene-paths
`Stroke`, every viewer draws it with the code that draws drawn paths, and it
is shared the way they are.

- **Seen by everyone.** `plan` saves the route into the scene's paths, so
  Home's page and every phone show it. A request may say `"share": false` to
  get the route back without saving it.
- **One route per caller.** A caller's new route replaces its previous one, so
  re-planning while walking does not pile up stale routes. Scene paths can only
  be appended to, so replacing means removing the old path and adding the new.
- **`clear`.** A second Operable (`spatial-harness.route-clear/v1`, just the
  `frame`) removes the caller's route when it arrives or gives up.
- **Over the peer link only.** Like `scene-paths` `add` and `remove`, this is
  a Component operation between authenticated peers, not a web request.

The caller is the authenticated peer that made the call, so one peer has one
shared route: a phone showing the way between two notes is not also being led
to one.

## One request, not a stream

`plan` is one-shot. A caller that is moving calls again as it moves. The phone
asks every second or two with `"share": false` to know which way to head, and
asks for a shared route only when it has left the one on the map by a metre. A retained `route` Product per caller is a
possible later addition if viewers need to watch someone else's route.

## Planner

A grid search over the scene's occupied voxels, in cells of the voxel size.
Voxels from 0.25 m up to `agent.height_m` are in the way and are widened by
`agent.radius_m`, so the agent does not go through a gap narrower than itself.
One cell beyond that is passable but avoided, and floor with a voxel below
0.25 m (seen floor) is preferred to floor with none. The ends stay reachable
when they are in or beside something, as a note on a wall is. `plan` in
`src/scene_navigation.rs` and `tests/scene_navigation.rs` describe it.

The phone's own planner (`MessageRoute.plan`,
`ios/PortalViewer/App/PortalMap.swift`) differs in one way: it treats the
radius as a preference, not a limit.

The SDK's `labs/auki-navigation` (navigation-mesh bake and path queries) is
the better engine once a scene has a mesh. The Operable's contract does not
change when the engine does.

## What the phone does

1. Call `plan` on `scene-navigation:<scene id>`. The route comes back as a
   path and arrives in the scene's paths like any other.
2. If Home does not advertise it, or the call fails, use the local planner,
   save the route with `scene-paths` `add`, and do not ask Home again for half
   a minute. A Home built before this exists keeps working.

Note-to-note routes are the same call with two `note` places.

## Not built yet

- Planning on the `voxel-to-occupancy-2d` layer, which knows free floor from
  unknown; the planner reads raw voxels.
- Dropping the route of a caller that has gone. Its path stays until someone
  erases it.
- Every call reads the whole voxel map. That is fine at a few calls a second
  on a room; a large map with many walkers will want a cached grid.

## Open questions

- Should `plan` accept several goals and return the nearest, for "take me to
  the closest unread message"?
- Does a robot need headings and a clearance profile in the result, or are
  floor points enough for its own controller?
- Floors: the planner assumes one walkable level at Y = 0 of the scene frame.
  Stairs and mezzanines need a level in the request.

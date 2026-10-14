# Navigation as an Operable Component

Status: proposal. Nothing here is built except the phone's own stand-in planner.

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
  the merged map and its `voxel-to-occupancy-2d` layer, which knows occupied,
  free and unknown floor. A phone only knows occupied voxels.
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
| `agent` | optional | `radius_m` and `height_m` of what is walking or driving. Defaults to a person. |

A place is either `{"point": [x, y, z]}` in `frame`, or `{"note": "<note id>"}`
for a note in the scene. Naming a note lets Home use the note's current
position, so note-to-note routes follow a note that is moved.

Result `spatial-harness.route/v1`:

| Field | Type | Meaning |
| --- | --- | --- |
| `frame` | string | The frame the points are in; equals the request's. |
| `status` | string | `found`, `direct` (nothing known in the way) or `unreachable`. |
| `path` | scene-paths `Stroke` | The route as a path: ID, title, color, width and one segment of points on the floor, both ends included. Absent when unreachable. |
| `length_m` | number | Length along the route. |
| `map_timestamp_ns`, `clock` | | Capture time and clock of the map the route was planned on. |

Rules, from `AGENTS.md`:

- Frames are declared, never inferred. A request for a frame that is not the
  scene's current frame is refused, not relabelled.
- A route is valid for the map it was planned on. `map_timestamp_ns` lets a
  caller tell a fresh plan from a stale one.
- Unknown floor is not free floor. The planner may cross unknown floor at a
  cost, and says so with `status`; it never reports it as clear.

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
- **`clear`.** A second Operable removes the caller's route when it arrives or
  gives up. Home also drops a route whose caller has gone.
- **Over the peer link only.** Like `scene-paths` `add` and `remove`, this is
  a Component operation between authenticated peers, not a web request.

Today the phone does this itself: it plans locally, saves the route with
`scene-paths` `add`, keeps it while the user stays within a metre of it, and
removes it with `remove` when it re-plans or stops.

## One request, not a stream

`plan` is one-shot. A caller that is moving calls again as it moves; the phone
re-plans every second or two today. A retained `route` Product per caller is a
possible later addition if viewers need to watch someone else's route.

## Planner

Start with a grid search over the occupancy-2d layer: obstacles are occupied
cells, widened by `agent.radius_m`; observed free floor is preferred to
unknown. This is what the phone does today on raw voxels, in
`MessageRoute.plan` (`ios/PortalViewer/App/PortalMap.swift`), and its tests
describe the expected behaviour.

The SDK's `labs/auki-navigation` (navigation-mesh bake and path queries) is
the better engine once a scene has a mesh. The Operable's contract does not
change when the engine does.

## What the phone does

1. Look for `scene-navigation:<scene id>` in Home's Catalog.
2. If it is there, call `plan`. The route comes back as a path and arrives in
   the scene's paths like any other.
3. If it is not, or the call fails, use the local planner. A Home built before
   this exists keeps working.

Note-to-note routes are the same call with two `note` places. Until Home has
the Component, the phone plans them locally from the notes' positions in the
scene it last received.

## Open questions

- Should `plan` accept several goals and return the nearest, for "take me to
  the closest unread message"?
- Does a robot need headings and a clearance profile in the result, or are
  floor points enough for its own controller?
- Floors: the planner assumes one walkable level at Y = 0 of the scene frame.
  Stairs and mezzanines need a level in the request.

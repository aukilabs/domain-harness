# 2D occupancy mapper

The `voxel-to-occupancy-2d` SDK Component consumes a scene Product and publishes a separate current-state scene as Product `occupancy-2d`. The output contains only an `occupancy_2d` layer. There are no scene, layer, or cell revisions.

The current harness exports voxel checkpoints inside complete scene Products. The input binding therefore accepts `spatial-harness.canonical-scene/v1` and projects its `voxels` field. It does not yet consume a standalone voxel-layer Product. The same retained-Product binding works locally on Home or with a remote subscription on any peer.

## Try it without a robot

### Godot robot peer

The [home playground](simulation/home/README.md) runs `voxel-to-occupancy-2d`
on the robot peer itself, with a local SDK input from `simulator-local-scene`.
Home discovers its separate `occupancy-2d` Product automatically under Network
maps. This uses raycast LiDAR observations, not the simplified patrol below.
Projection runs at 1 Hz; the known flat world floor is expressed in the robot's
boot frame. Neither map is merged into Home's canonical scene.

### Moving robot in the real Home UI

```sh
env -u SPATIAL_PEER_CONFIG SPATIAL_SIMULATED_ROBOT=1 SPATIAL_PORT=4323 cargo run --locked --bin spatial-home
```

Open [Home](http://127.0.0.1:4323), select **Maps**, then **Simulated robot · voxels** or **Simulated robot · occupancy 2D**. Use Top for a plan view; click geometry to inspect individual voxel/sample values. The occupancy legend is coral = occupied, green = free, slate = unknown; absent cells are unknown too. The existing voxel-layer checkbox controls either visualization. Inspect/export downloads the native scene JSON, not a rendered image.

By default this starts **one robot peer and one Home peer**. Set `SPATIAL_SIMULATED_ROBOT_COUNT=2` for the two-robot collaborative demonstration described below; only 1 or 2 is accepted. Each robot exports `fake-voxels-scene`, and Home subscribes to its observations. Home publishes a combined voxel Product, which drives `occupancy-2d`. All peers run **in the Home process**, with distinct identities and loopback listeners. They use an isolated authority, not DDS credentials. No discovery, external services, real robot commands or production peer connections are used. The simulator rejects `SPATIAL_PEER_CONFIG`. Stop with Ctrl-C; restart after 30 minutes when its demo credentials expire.

Robot 1 and Robot 2 start on opposite sides of the same patrol, with separate colored pose markers in both views. Each supplies its own 1.5 m observation footprint. Home merges matching simulated-time rounds only; simultaneous conflicts conservatively prefer occupied evidence. The source snapshot records both contributor peer IDs and poses. Both robots use a known shared frame: this tests multi-peer observation fusion, not localization or calibration between independent frames. If either peer disconnects, simulation stops updating and retains its last map with failed/stale status.

In **Devices**, expand Robot 1 or Robot 2 to inspect its peer ID, real SDK Component and Product manifests, and live received-observation count. Entries come from Home's SDK known-peer inventory; robot catalogs are fetched through authenticated `catalog_exact` requests. The count increments only when Home receives a Product observation over P2P. Home's own catalog includes the occupancy mapper. These peers run in one process with independent SDK runtimes, identities and loopback listeners, not separate OS processes. Synthetic scene data is produced by configured observables and retained Products, not injected by the browser.

Use **Devices → Disconnect simulated robots** to shut down the robot peers while leaving Home, its occupancy Component and retained Product running. This calls the localhost-only `POST /api/simulation/disconnect`; it is idempotent. The voxel view becomes unavailable and its last observations are removed from the API. Occupancy remains selectable as **retained**, with a banner explaining that publishers are offline; its input freshness becomes stale without erasing map cells. A robot transport closure also disconnects the simulation sources while preserving Home. Restart the simulator to reconnect robots. Stopping Home itself still loses its in-memory map. Browser request failures explicitly label the last rendered view as an offline cache and disable scene selection until Home responds again.

The robot patrols a 1.8 m radius circle once per minute in a 6 × 6 m synthetic room. Every second it publishes **only its current 1.5 m horizontal observation footprint**, not accumulated coverage. Samples leaving view become unknown, never free. Walls, a central 80 cm block and floor are synthetic. A second block (60 cm wide, 80 cm tall) shuttles along Z near X = 1.2 m every 12 seconds. While observed, its new location becomes occupied and its old location is explicitly observed free. This gives repeatable occupied→free, free→occupied and observed→unknown transitions even after many patrols.

This footprint is deliberately simplified, not a LiDAR raycaster or collision/navigation simulator. The north-east corner remains unknown. Home projects the first meter above floor Y = 0 at 1 Hz, using 10 cm samples and 2 m cells. The robot pose is shown in both views; it is telemetry, not an obstacle. The voxel view shows current observations only, while the occupancy view accumulates known space across the patrol. Leaving the footprint does not erase occupancy; observed free evidence clears an old obstacle.

Synthetic scenes are in-memory and never merged into Home's canonical map. The Home journal defaults to `data/simulated-robot/home.json` in this mode (override with `SPATIAL_DATA`); restarting resets the patrol and surveyed area. Transport failure leaves the last map visible with a failed/stale status rather than silently continuing.

### Small component-only demo

```sh
cargo run --locked --bin occupancy-demo
```

Open [the local demo](http://127.0.0.1:4321). No credentials, robot, or Auki services are required. Use `-- --port 4322` to choose another port.

An explicit `fake-voxel-source` Component publishes a deterministic synthetic voxel scene through an SDK retained Product. The real occupancy Component binds that Product and runs at 1 Hz. This exercises local Component/Product delivery and projection; it does not simulate network transport. The existing authenticated peer tests cover remote delivery separately.

Choose a low block, remove it, put it overhead, or remove observations from the center. Compare the illustrated source layout with actual mapper output. Pause the source for more than five seconds to test stale status; resume to recover. The upper-right corner is always unobserved, and the floor stays below the obstacle band. The API at `/api/state` exposes the current occupancy scene as JSON.

The demo binds only localhost, keeps state in memory, and never writes into Home's map or connects to deployed peers. Stop it with Ctrl-C.

## Run on Home

Set `SPATIAL_OCCUPANCY_FLOOR_M` to the measured flat floor height in the Home live map, then start Home through its usual authenticated configuration. For example, **only if the floor is at height zero**:

```sh
SPATIAL_OCCUPANCY_FLOOR_M=0 SPATIAL_PEER_CONFIG=config/home-peer.example.json cargo run --locked --bin spatial-home
```

The mapper binds Home's `home-live-scene` Product, targets 1 Hz, and exports `occupancy-2d` on the same peer/catalog. `/api/peer` includes its status and Product ID. Without the floor environment variable, the mapper is disabled. A portal-centered map origin is not necessarily the floor; do not infer floor height from the supplied zero portal heights.

## Run on another peer

Use that peer's own identity/configuration and the source's connection profile:

```sh
cargo run --locked --bin occupancy-mapper -- \
  --config data/occupancy-peer.json \
  --source data/home-connection.json \
  --product home-live-scene \
  --floor-m 0 --hz 1
```

Supply normal authentication environment variables as with other harness peers. `--floor-m` is mandatory. Use `--product canonical-scene` for a runner's independently built map. The standalone process stops on transport failure; restart it to rediscover and bind the current Product. It does not issue robot commands or alter source data.

## Projection contract

- Input voxel resolution: 10 cm; other resolutions are rejected.
- Flat floor: explicit height along the positive up axis declared by the source frame. Y-up and Z-up frames work without guessing coordinates.
- Obstacle band: voxel **centers** strictly above the floor and at most 1 m above it. A center on the floor is excluded; one exactly 1 m above it is included. Floor-straddling voxels whose centers lie above the plane count as obstacles; calibrate the floor with this discretization in mind.
- Occupied: any positive occupancy evidence in the column's ten sampled heights.
- Free: negative evidence at all ten heights and no occupied evidence.
- Unknown: missing or zero evidence. Sparse input can therefore yield mostly unknown navigation space; absence of a visible voxel is never treated as free.
- Output resolution: 10 cm. Each 2D cell covers 2 × 2 m with 400 signed values, row-major `v * 20 + u`: `-1` unknown, `0` free, `100` occupied.
- The plane's source frame, horizontal axis indices, floor height, and UV origin are explicit. In Y-up frames UV uses source X/Z; in Z-up frames it uses X/Y. A robot format adapter must respect these declarations.
- Cell coordinates use floor division, including negative positions. Internal 1.6 m voxel chunks do not dictate output cell boundaries.
- Output is bounded to 8,192 non-unknown spatial cells. Oversized or invalid input reports `invalid_input` and preserves any prior map as cached data.

The mapper accumulates occupancy in memory for its lifetime. New occupied or fully observed free samples replace previous values; unknown samples and missing input cells leave prior values untouched. There is no time decay. Clearing an obstacle requires explicit free evidence across the full height band; absence of voxels is not sufficient. Known space grows as new areas are observed. Restarting the mapper resets this in-memory map (no disk persistence yet).

Published output is still a complete current occupancy scene, not a delta: consumers replace their prior output with it. Missing cells in that accumulated output have never been observed. Frame/grid geometry changes are rejected rather than combined with incompatible history. The accumulated map is capped at 8,192 cells; a growth update exceeding that cap is rejected atomically with `invalid_input`, preserving the prior map. There is no inflation by robot footprint; this is occupancy evidence, not a robot-specific costmap.

The Component also exposes authenticated `read_cells`, accepting `{ "cells": [[0,0], [-1,2]] }` with 1–64 entries. It returns a `CellSelection` explicitly marked `complete_scene: false`; only the requested cells should be replaced by the caller. Requested cells absent from an available map contain unknown values. A waiting result has no occupancy layer. Discover the operation through the peer Component catalog; the source and output use the existing Domain authentication boundary.

## Cadence and availability

The mapper retains only the latest pending input and rate-limits processing to the configured cadence (default 1 Hz). Calls execute sequentially, so mapping cycles cannot overlap. Unchanged output is not republished. After five seconds without a valid source observation, default output status becomes `stale`; the last map remains explicitly cached. Source closure reports `unavailable`, invalid input reports `invalid_input`, and no first input reports `waiting`.

Staleness measures receipt of source publication, not synchronized sensor capture time. A quiet source can be marked stale even when its map is unchanged. Consumers must inspect status. A cycle can exceed its target period on sufficiently large inputs; 1 Hz is a target rather than a hard real-time guarantee.

Home's scene selector renders the explicitly enabled simulator's voxel and occupancy outputs. General remote occupancy-Product discovery, robot-format export and planner integration remain separate work.

## Validation

```sh
cargo test --locked --test occupancy_mapper
cargo test --locked --test simulated_robot --test occupancy_demo
cargo check --locked --bin occupancy-mapper --bin spatial-home
```

Tests cover vertical bounds, explicit floor offsets, Y/Z-up frames, negative cell addressing, free/unknown distinction, obstacle replacement, selected-cell reads, rate limiting, stale state, and authenticated remote input/output.

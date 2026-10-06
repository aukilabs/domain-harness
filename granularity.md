# Granular scenegraph retrieval

Status: working design note for discussion. This is not an implementation plan or a settled schema.

## Direction: retrieve by layer

The first requirement is to request a named kind of scene content: “give me the portals,” “give me the voxels,” or “give me the mesh.” A layer is an independently discoverable and retrievable collection with a declared type and frame. Consumers can request one layer or combine several in the same scene.

Conceptually, the scene exposes:

```text
Scene
  shared metadata: identity, authority, root frame
  layers
    portals → marker identities, sizes, and poses
    voxels  → occupancy grid and measured color
    mesh    → surface vertices, triangles, and optional appearance
    objects → semantic objects and their placements
    notes   → spatial annotations
```

These are logical content layers. Their representation can differ: portals are a collection of anchors, voxels are a grid, and a mesh is surface geometry. They need a common discovery and retrieval envelope, while retaining their own typed payloads. The current aggregate has no dedicated mesh layer; that is a proposed extension, and producing mesh geometry is a separate capability from retrieving it.

The initial conceptual interface is:

```text
list_layers(scene)
get_layer(scene, "portals")
get_layer(scene, "voxels")
get_layer(scene, "mesh")
get_layers(scene, ["portals", "mesh"])
follow_layer(scene, "voxels")
```

These names describe desired behavior, not existing APIs. A portals request transfers the portals and the small amount of metadata needed to interpret them. A mesh request transfers mesh content and its metadata. Merely hiding other layers after downloading the entire scene would not satisfy this requirement.

Each layer descriptor should identify its stable layer ID, content type/schema, frame, availability, and Product reference. Discovery should return these descriptors without loading layer payloads. An available but empty layer is distinct from an unsupported layer, a layer still being built, or one temporarily unavailable.

Layers share a scene coordinate system or declare an explicit transform into it. A client can therefore overlay portals and mesh without also fetching voxel data. A mesh generated from voxels can lag behind the latest voxel layer. Existing producer provenance can describe its inputs without requiring the reader to download them or coordinating publication across layers.

For a first version, use one conventional ID per type (`portals`, `voxels`, `mesh`). Keep ID and type separate in the model so a scene could later contain two mesh layers, such as a measured surface and an authored model. Multiple layers of the same type would require an explicit selection or a declared default.

A small scene directory lists layers and their Products. Each layer is exposed as its own Product. Spatial reads select cells within that Product; they do not create a Product per cell. The operation contracts still need design. Entity lookup and deltas can follow later.

## Agreed model: scenegraphs have current state, not revisions

The scenegraph is mutable current state. Its layers, cells, and entities do not have application-level revisions or version histories. Updates replace current content under stable identities. This is the scenegraph model, not merely the omission of a global revision counter.

Reads select a Product and cells and return their current available content. Layers update independently. There are no revision parameters, historical scenegraph lookups, or requests for a compatible set of past layer states. The directory describes what is available and how to retrieve it.

Existing SDK observation sequences may remain transport bookkeeping for delivery and ordering. They must not become scenegraph, layer, cell, or entity revision identifiers. Schema versions describe the data format, not revisions of scene content. Existing SDK snapshot-reference mechanics belong to the implementation baseline and are not a requirement to expose versioned scenegraph state in this design.

The accepted tradeoff is that a combined view can contain data of different ages. Frame validation and explicit availability still apply. Following a gap or reconnect, clients reread the current selected content. Historical reconstruction is not part of this scenegraph model.

## Agreed direction: standard and custom layer types

Use an open layer system with Auki-defined standard types and developer-defined custom types. Both have precise, versioned payload contracts. The set of types is extensible; a layer's contents still conform to its declared schema.

Auki maintains standard contracts and first-party tooling for core types, with portals, voxels, and mesh as the initial candidates discussed here. Developers can introduce types such as traversability, temperature, inventory, or acoustic measurements without changing the scene's common envelope.

Every layer shares a descriptor containing its layer ID, namespaced type/schema identity and version, Product reference, spatial frame where applicable, and supported retrieval operations. Spatial layers supporting cell retrieval use the scene's shared 2-meter grid. The payload remains specific to the declared type; non-spatial content should not be assigned artificial cell coordinates.

Discovery and interpretation are separate capabilities. Any consumer implementing the common envelope can discover a custom layer and determine its retrieval capabilities. Interpreting, rendering, or processing its payload requires support for that schema. An unfamiliar type can be skipped without preventing use of recognized layers.

Keep layer instance IDs separate from type identities. Multiple developers can define types without naming collisions, and multiple layers can share one type. A custom type may later inform an Auki standard through an explicit version or migration mapping; existing schema identities must retain their meaning.

A Component publishes the custom Product through the same infrastructure as standard Products. The scene authority controls whether that Product is included as a layer in its scene. Publication alone does not automatically grant scene membership. Exact descriptor fields, schema naming conventions, and registration operations remain to be designed.

## Starting decision: 10 cm voxels, 2 m cells

Use 10 cm voxels and cubic 2 × 2 × 2 meter retrieval cells. Each cell spans 20 × 20 × 20 voxel positions: 8,000 possible voxels, with sparse storage where appropriate. Power-of-two dimensions are not a requirement. We can revisit these sizes after measuring actual workloads.

Use **cell** for the shared spatial retrieval unit, **voxel** for an occupancy sample, and **chunk** for an internal storage unit. The existing SDK's 16-voxel chunks are 1.6 meters across at this resolution. Serving 2-meter cells therefore requires selecting and assembling data across internal chunk boundaries; it does not require changing the mapper's chunk format.

Proposed addressing contract: the scene declares a grid ID, frame, origin, and cell size. Products use that common grid so the same cell IDs select corresponding portions of portals, voxels, mesh, and other spatial layers. With aligned voxel and cell origins, cell coordinates are computed per axis as `floor((position_m - origin_m) / 2)`. Use half-open bounds and mathematical floor, including for negative coordinates.

Conceptually:

```text
read_cells(product="mesh", grid="scene-grid-id", cells=[(3,0,2), (3,0,3)])
```

This is a proposed operation, not an existing SDK API. A request for geometry within five meters of a pose selects all cells intersecting that sphere. Returned cells may extend beyond the sphere. Responses identify the Product and returned cells and distinguish available-empty cells from unavailable or unobserved content; no geometry is not proof of free space.

Cache current results by Product identity, grid identity, and cell coordinates, replacing selected cell content when fresh data arrives. Cache freshness and invalidation details remain to be designed without requiring scene revisions. Geometry spanning cell boundaries must be included wherever it intersects: whether to split mesh geometry or reference shared pieces with stable IDs remains open.

## The question

How should we structure the scenegraph so a consumer can retrieve only the layers and cells it needs, with enough type and frame information to interpret them?

“More granular” can mean several different things:

- one entity rather than every entity;
- one kind of data, such as portals or notes, rather than the whole scene;
- one spatial region rather than the whole map;
- updates to selected content rather than another full snapshot;
- current state rather than history or provenance;
- a cheap summary rather than full geometry.

We should decide which of these query shapes matter before choosing a storage representation. A more deeply nested JSON document is not, by itself, a more retrievable scenegraph.

## What exists today

There are two related structures in the current system:

1. The SDK `Scenegraph` is deliberately small: one map root and a flat map of QR anchors. A `MapSnapshot` carries that typed state plus a deterministic USDA projection. The SDK already supports QR lookup and payload search on the current map, with the exact snapshot reference returned alongside the result.
2. The harness `CanonicalScene` is an application-level aggregate. It combines the portal `MapSnapshot`, a complete voxel checkpoint, semantic objects, and an open-ended registry envelope. Text notes and mapping metadata currently live in that registry envelope.

The network Product publishes `CanonicalScene` as complete replacement state. It retains two observations and supports “latest existing” followed by new observations. This makes gap recovery simple: the next complete snapshot supersedes the previous one. It also means a consumer interested in one note, one portal, or a small patch of voxels still receives the entire scene, up to the current 24 MiB publication bound.

The HTTP surface exposes a few narrower views, but these are server-specific projections over the same aggregate state rather than stable granular scene resources.

Important existing invariants are worth preserving:

- the authority peer and publication clock are explicit;
- every spatial value names or inherits a validated frame;
- the portal map and voxel map are tied together explicitly;
- a voxel snapshot is a replacement checkpoint, not an additive update;
- sensor Products, capture clocks, and source identities remain provenance, not inferred global time;
- a missing entity is not silently interpreted as a deletion;
- remote scenes are validated before use and are not implicitly merged into Home’s scene.

## Queries we may want to support

The design becomes easier to evaluate if we write the consumer questions first.

| Consumer question | Required granularity |
| --- | --- |
| “Give me the portals.” | complete portals layer |
| “Give me the voxels.” | complete voxels layer |
| “Give me the mesh.” | complete mesh layer, when available |
| “Give me portals and mesh together.” | latest available data from selected layers |
| “Give me portal `UCOF1J40N7O`.” | entity by stable ID |
| “Give me all text notes anchored to this portal.” | relation plus entity kind |
| “Give me occupied geometry within 2 m of the robot.” | spatial region plus voxel layer |
| “Give me labels and poses, but no photos or voxel payloads.” | component or field projection |
| “Give me the exact inputs that produced this placement.” | provenance traversal |
| “Subscribe only to moving actors.” | layer plus change stream |
| “Render a coarse overview, then refine nearby geometry.” | level of detail plus region |

These query dimensions are independent. For example, “follow notes in these cells” combines entity kind, space, and updates. Entity queries, provenance traversal, and detail levels are possible extensions beyond the initial layer/cell reads.

## Design goals

A granular design should:

- avoid transferring or decoding unrelated large payloads;
- retain stable logical identity across updates;
- let a consumer combine selected layers using explicit types and frames;
- support independent update rates for static, dynamic, and high-volume data;
- make deletion, staleness, and partial availability explicit;
- keep frame, authority, schema, and provenance validation possible at every boundary;
- permit caching and deduplication;
- work over authenticated SDK Products and operations, not only the local HTTP server;
- preserve the current whole-scene Product during migration.

It should not require every datum to become a separate network Product. Very fine Products would create catalog churn, subscription fan-out, and difficult authorization bookkeeping.

## Proposed conceptual model

The working proposal is to separate four concerns that are currently bundled together:

1. **A scene directory** lists available layers and their Products.
2. **Typed resources** hold independently retrievable state.
3. **Indexes and query operations** locate resources by ID, type, relation, or space.
4. **Subscriptions** deliver updates to selected content; detailed cell subscription behavior remains to be designed.

The scene remains a graph at the logical level. It does not need to be serialized or transported as one recursively embedded object.

### 1. Scene directory

The directory should be small enough to fetch cheaply. It describes layer membership and retrieval without pinning their data to one moment in time.

Conceptually:

```json
{
  "scene_id": "<peer-scene-id>",
  "authority_peer_id": "…",
  "root_frame": "domain:…:home-origin-UCOF1J40N7O",
  "layers": {
    "portals": { "product": "portal-map" },
    "objects": { "product": "scene-entities" },
    "notes": { "product": "text-notes" },
    "voxels": { "product": "scene-voxels" },
    "mesh": { "product": "scene-mesh" },
    "presence": { "product": "scene-presence" }
  }
}
```

This is illustrative, not a proposed wire schema. Product strings abbreviate full SDK Product references, which identify the peer, Product, and exact Product manifest. Such a reference identifies the Product contract, not a frozen scene state. Readers request its current content; the directory does not name observation sequences to fetch.

### 2. Typed resources

The first useful split follows lifecycle, size, and access pattern rather than current struct boundaries.

| Resource | Natural unit | Typical update rate | Notes |
| --- | --- | --- | --- |
| Map definition and portals | map or portal entity | low | Alignment root; producers must handle changed alignment inputs when updating dependent geometry. |
| Semantic entities | entity or bounded page | low to medium | Objects, labels, poses, relations, and provenance. |
| Text annotations | note entity or bounded page | low | Notes should become first-class typed entities instead of an opaque registry entry. Large photos should be separately referenced blobs. |
| Voxel geometry | 2 m retrieval cell | medium | 20×20×20 voxel positions at 10 cm resolution; assemble from internal SDK chunks. |
| Surface mesh | mesh or mesh patch | producer-dependent | Proposed layer; declare geometry format, frame, and producer provenance where available. |
| Presence and robot pose | actor stream | high | Ephemeral telemetry should not force static scene republication. |
| Mapping metadata | typed status resource | medium | Operational state is useful, but it should not masquerade as durable scene content. |
| Provenance | observation/source references | append-oriented | Retrieve on demand unless a consumer explicitly asks for it. |

A resource should carry enough envelope information to be validated independently:

- schema and schema version;
- logical resource ID;
- authority;
- frame or an explicit non-spatial marker;
- lifecycle state (`present` or `tombstone`);
- optional spatial bounds;
- source and dependency identities, such as the portal Product used for alignment;
- provenance references or a summary of them.

### 3. Entities and components

For semantic content, an entity/component model gives finer projection without forcing a different logical graph.

An entity has a stable ID and one or more typed components:

```text
entity: note:4d2…
  kind: text-note
  pose: position in frame F
  annotation: text, title, description
  media: reference to photo blob
  relation: anchored-to portal:UCOF1J40N7O
provenance: writer-peer-id:"runner-bot-a" (SDK source_peer concept); optional data-origin tracking if different from peer who decided final canonical state
```

Candidate common components are:

- `pose` or `bounds`;
- `geometry` or an external geometry reference;
- `semantic` labels and properties;
- `relations` such as `anchored_to`, `contains`, `part_of`, or `observed_by`;
- `provenance`;
- `lifecycle` metadata.

Not every component needs separate storage. We can initially return a whole small entity while keeping large or independently authorized fields—photos, meshes, point clouds, voxel chunks—behind references. Field projection is only valuable if measurements show that small entity payloads are themselves a bottleneck.

The transform hierarchy and the retrieval graph should also remain distinct. `parent_frame` answers how to compose a pose. Relations such as “note is about portal” or “object is inside room” are semantic edges and must not change spatial math accidentally.

### 4. Spatial resources

Spatial layers expose the 2-meter cells defined above. A cell selection is identified by:

```text
(product_reference, grid_id, cell_x, cell_y, cell_z)
```

An index can map a requested bounding box, radius, or view frustum to cell references. Cell bounds are expressed in the declared grid frame. A request in another frame needs an explicit, validated transform before intersection testing.

Keep one Product per layer and expose a bounded cell-read operation on its serving Component. Possible later additions are:

- a lightweight index of available cells;
- a subscription reporting changes to selected cells;
- detail levels within a cell, if needed.

Start with full replacement content for each returned cell. Internal storage chunks remain an implementation detail. A response does not imply that other layers or separate cell requests represent the same instant.

### 5. Indexes and query operations

Indexes are derived access paths, not additional authorities. Query results identify their Product and selection and return current content, resource references, or compact summaries.

Useful operations are likely to include:

```text
get_entity(product, entity_id, projection?)
query_entities(product, kinds?, bounds?, relations?, page?)
read_cells(product, grid_id, cell_ids[])
```

Pagination and entity queries are deferred. If introduced, they must state how concurrent changes affect enumeration; no scene-wide snapshot isolation is promised. The first cell-read API should bound request and response sizes and explicitly identify cells that were not returned.

Spatial, type, relation, and provenance indexes can evolve independently as derived data. The canonical entity/resource records remain the source of truth.

### 6. Updates and recovery

Layer Products publish independently through their existing observation mechanisms. Selected-cell subscriptions could later send full replacement cell content or notifications to reread affected cells. SDK observation sequences can order updates within a Product; there is no global scene sequence.

On a gap or reconnect, reread the current selected cells or layer. Delta encoding, replay history, and coordinated publication across layers are outside the first slice.

## Identity and deletion

Keep logical identity stable across updates: correcting a portal's pose does not give it a new portal ID. Product manifest identity and schema versions retain their existing SDK meanings.

A complete cell response replaces the client's cached content for that cell, including an explicit empty result when appropriate. Omission from a partial or failed response does not clear a cell. Removing a cell from a subscription's selection means it left the selected region, not that its contents were deleted from the Product. Entity-level deletion events and tombstone retention can be designed if incremental entity updates are introduced.

## Consistency for the first version

Use independent current-state layer and cell reads. Consumers may see freshly updated voxels alongside a mesh that has not yet been regenerated. Producers must still provide valid typed payloads and explicit spatial contracts. Existing mapper provenance checks may remain internal; they do not impose revisions on the scenegraph model or retrieval API.

## Authorization and trust boundaries

Granularity can improve access control, but only if authorization aligns with resource boundaries. Examples include allowing a peer to read public geometry but not note photos, or to follow robot presence without receiving mapping provenance.

Queries must enforce authorization before filtering results; otherwise counts, bounds, or IDs can leak hidden entities. The directory should either omit inaccessible resources or mark them as intentionally unavailable without revealing protected metadata. We should not assume that Domain membership alone grants the same rights to every resource or operation.

Writes are a separate concern from reads. A granular read model does not imply arbitrary per-entity mutation. Home can remain the canonical writer and expose narrowly scoped operations, as it currently does for annotations and mobile depth.

## Options considered

### Keep one aggregate and add HTTP filters

This is the smallest experiment and could validate query shapes. It does not reduce P2P scene publication, allow independent subscriptions, or create stable resource references. It is useful as a prototype, not a sufficient end state.

### Publish one Product per layer

Portals, objects, notes, voxels, mesh, and presence update independently, with a directory listing their Products. This directly fits the first requirement: retrieve or follow a selected kind of content. Cell selection refines retrieval inside spatial layers.

### Publish one Product per entity or chunk

This maximizes subscription choice but likely creates too many catalog entries, Products, and subscriptions. It also makes coherent enumeration difficult. Prefer bounded lookup operations or paged resources unless SDK measurements show that high Product cardinality is cheap.

### Adopt a property-graph database as the canonical format

A property graph could answer relation queries well, but it does not by itself solve spatial partitioning, P2P publication, or frame validation. It may be a useful server-side index later; it should not define the transport contract prematurely.

### Use USDA composition as the retrieval mechanism

USD layers and payloads are attractive for scene composition and rendering. The current USDA is explicitly a deterministic projection of typed state, though, and the voxel and provenance contracts contain semantics that generic stage composition does not guarantee. We should keep USD as an export/view format unless we intentionally redesign authority and validation around it.

## A practical first slice

Before choosing a final schema, measure real scene size and update frequency by current field. Then try the smallest split that exercises the hard parts:

1. Keep `canonical-scene` unchanged for compatibility and recovery.
2. Add a small layer directory with typed descriptors and Product references.
3. Expose independently retrievable portals and voxels layers from existing state. Extend the same contract to objects and notes.
4. Define the optional mesh layer contract, with explicit availability until a mesh producer exists. Decide mesh generation separately.
5. Support selected-layer reads and define bounded reads for selected 2-meter cells. Keep 10 cm voxel resolution. Defer entity queries and deltas until needed.
6. With fixed test input, verify that selected layer/cell reads return the corresponding content from the existing aggregate and clearly identify partial or unavailable results.
7. Measure payload reduction, round trips, cache behavior, catalog size, and recovery by rereading current content.

A successful first slice should answer:

- Can a consumer retrieve portals without transferring or decoding voxels, and retrieve voxels without unrelated annotations?
- Can a consumer discover whether mesh is available and overlay it with portals using declared frames?
- Can a new or recovering client fetch the current selected layers and cells?
- Can a portal change invalidate dependent geometry explicitly rather than silently?
- Can a dropped update recover by rereading current content?
- Does the Product/operation split remain manageable with multiple runners and the phone?

## Use case: voxel-to-2D-occupancy mapper

Implementation: see [occupancy-mapper.md](occupancy-mapper.md). The first Component binds existing complete scene Products and extracts their voxel checkpoints, since independent voxel-layer publication is not yet available. It publishes a separate typed occupancy scene, supports selected-cell reads, and can run on Home or a standalone peer. General layer infrastructure remains separate work.

Create a mapper Component that consumes the voxel Product of one scenegraph and publishes a 2D occupancy Product in a separate output scenegraph. The initial deployment runs on Home, but the Component should run on any peer with access to its input Product. Product bindings and configuration supply its inputs; it must not depend on Home's in-process scene store.

```text
Source scenegraph / voxels Product
  → voxel-to-2D-occupancy Component
  → output scenegraph / occupancy-2d Product
```

The output scenegraph has its own identity and declares the output plane's relationship to the source frame. It need not copy portals or source voxels. Both input and output represent current state without scenegraph revisions. A robot-specific format adapter can consume the occupancy Product separately.

### Initial geometry and classification

Assume a flat floor. Configure its height and up direction explicitly in the source frame; a map's origin is not automatically the measured floor. Treat occupied voxel evidence within the first meter above that floor as an obstacle. The intended band is `0 < height_above_floor <= 1 m`, excluding the floor surface itself.

The initial implementation classifies voxel centers using `(floor, floor + 1 m]`. A floor-straddling voxel whose center is above the configured floor counts as an obstacle. This rule is explicit and tested; any tolerance for noisy measured floor surfaces needs later calibration.

Proposed output resolution is 10 cm. For each horizontal grid location:

- **Occupied:** at least one relevant voxel in the height band has occupied evidence.
- **Free:** the required vertical band has sufficient observed free-space evidence and no occupied evidence.
- **Unknown:** neither condition can be established, including missing coverage.

The initial implementation requires negative evidence at all ten sampled heights to declare free space; zero or missing evidence remains unknown. It reads the full voxel checkpoint, including free-space evidence, rather than only occupied visualization geometry.

Proposed spatial retrieval uses 2 × 2 m cells on the output plane, with 20 × 20 occupancy values per cell. This is a proposed 2D extension of the shared cell scheme; its grid descriptor must declare its dimension, plane, axes, and origin. Source 3D cells intersecting the height band supply each output cell.

### Scheduling: 1 Hz

Target one mapping cycle per second, configurable, using the latest available source data. Coalesce updates received between cycles and avoid overlapping mapping cycles or building a backlog. If a cycle overruns, report that condition and process current input on the next available cycle rather than replaying obsolete intermediate states.

A 1 Hz target is plausible for a bounded local voxel map, but is not yet benchmarked. At 10 cm resolution, an aligned 2 × 2 × 1 m region contains 4,000 possible voxel positions to classify. Transfer, decoding, input extent, and output publication can dominate that simple projection. Measure the complete cycle on Home and another target peer before treating 1 Hz as a guarantee. Incremental recomputation of affected cells is an optional optimization, not a requirement for the first implementation.

Publish changed current output after each completed cycle. An unchanged map need not be retransmitted simply to prove the Component is alive; status can report the last successful cycle and input availability separately. Changed input must allow output cells to become free or unknown as well as occupied. On unavailable input, retain any cached result with explicit stale/unavailable status rather than inventing free space.

One-hertz processing can add up to roughly one second of scheduling delay before transfer and processing time; input may already be older. This is a navigation map update target, not a claim about immediate obstacle avoidance. The robot's map format, footprint/inflation handling, and freshness requirements still need to be established.

## Questions for us

These decisions materially change the design:

1. Which consumer is driving this work first: robot reasoning, AR rendering, browser visualization, offline export, or something else?
2. The initial queries are portals, voxels, and mesh by layer. What should the mesh represent, and which component would produce it?
3. Given independent latest reads, what freshness information does the first consumer need?
4. Cell IDs are the initial spatial selection contract. Which convenience queries should translate into those IDs first: radius, bounding box, or frustum?
5. Are notes, detected objects, portals, and robots all first-class entities, or do some remain projections/status?
6. Which layer or cell subscriptions are useful after current-state reads work?
7. Must granular retrieval work directly peer-to-peer while Home is offline, or is Home always the query authority?
8. Which data needs different read permissions, particularly photos and provenance?
9. How should cached cells be refreshed and unavailable cells retried?
10. Should semantic relations be canonical authored facts, derived index facts, or both with explicit provenance?

## Current working position

The strongest starting hypothesis is:

> Expose the scene as discoverable, typed layers that can be retrieved and followed independently. Start with portals and voxels, and define mesh as an optional layer. Use a small directory to describe their Products, frames, and availability. Read current data by layer or selected cells, without scene revisions.

The initial retrieval boundaries are layers and their 2-meter cells, with 10 cm voxels. Entity queries, detail levels, and deltas remain later refinements. The operation contracts must preserve validated scene composition and distinguish partial results from complete layer state.

## Decision log

Use this section as the discussion converges.

| Date | Decision or experiment | Rationale | Status |
| --- | --- | --- | --- |
| — | Initial manifest with coordinated resource revisions | Earlier exploration of exact scene snapshots | Superseded by independent current-state reads |
| 2026-10-05 | Make portals, voxels, and mesh selectable layers | User's stated retrieval model | Requirement clarified; transport design open |
| 2026-10-05 | One Product per layer; select spatial content by cells | Shared addressing across layers | Agreed direction; operation contract open |
| 2026-10-05 | Start with 10 cm voxels and 2 m cubic cells | Optimize after establishing the retrieval model | Decided |
| 2026-10-05 | Open layer types with Auki standards and developer-defined schemas | Common discovery and retrieval infrastructure, precise contracts for both | Agreed direction |
| 2026-10-05 | Scenegraphs are mutable current state, without scene, layer, cell, or entity revisions | Stable identities and current-content reads; transport ordering remains separate | Decided; clarified |
| 2026-10-05 | Voxel-to-2D-occupancy mapper produces a separate scenegraph; runs on Home initially and can run on any peer | Navigation use case through Component/Product bindings | Agreed direction |
| 2026-10-05 | Flat floor; obstacles in the first meter above ground; target 1 Hz mapping | Initial navigation projection and update cadence | Decided; boundary rule and performance measurement pending |

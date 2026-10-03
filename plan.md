We are going to create a tech demo of the Auki SDK, where we create a general purpose spatial harness for robots.

We will use [Auki SDK PR #421](https://github.com/aukilabs/auki-sdk/pull/421) and its component-based architecture.

Scope and constraints:

- Create P2P clusters and work with scenegraphs.
- Auki relay servers may be used to reach peers without an exposed IP address.
- Leave existing portal infrastructure untouched.
- Make no calls to Auki backend services about portals, including automatic metadata or size-lookup fallbacks.
- Inject portal sizes manually. The 13 supplied Portal codes and physical dimensions are recorded in [config/portals.json](config/portals.json), in meters, confirmed as encoded-square sizes excluding the white border. The supplied poses are retained in [config/domain-portals.json](config/domain-portals.json). Both these Portals and the synthetic scene use the OpenGL Y-up frame of domain `b2881524-954d-4979-b715-240ed8f02807`. All stored Portal heights stay at zero; mounting heights remain unverified. Backend UUID mapping remains open.

The demo has three application peers:

**Annotator App**
An iPhone augmented reality app peer that uses the SDK to create a scenegraph of portals and other semantic objects. It also builds physical topography into the scenegraph as a voxel map using LiDAR or other data from ARKit.

The voxel map must support raycasting so detector components can project screen-space detections into 3D scenegraph positions. For example, a YOLO detector identifies books on a table; rays from the corresponding camera image intersect the voxel map to place book objects in 3D.

Component plan: [annotator.md](annotator.md).

**Home Server**
A peer you can run on your home computer that builds, stores, and serves the canonical scenegraph. It ingests both observations and scenegraphs from other peers, integrating their contributions into the canonical scenegraph, including its voxel map. The Home Server also runs a simple browser-accessible 3D visualizer.

Component plan: [home.md](home.md).

**Bracketbot Runner**
A peer on the Bracketbot robot that consumes peer data and contributes its own sensor observations to the shared voxel map. Both the Annotator and Bracketbot must be able to contribute topography in a common scenegraph coordinate frame.

Component plan: [runner.md](runner.md).

## Expected components

These are proposed responsibilities, not confirmed SDK class names or a fixed one-to-one implementation. Each peer's plan expands its list.

- **Shared peer runtime and exchange:** component discovery, input/output bindings, P2P transport with optional relays, and the frame, clock, and source metadata needed to interpret contributions.
- **Annotator:** ARKit source, Portal detection/localization, local mapper, object detector, voxel raycaster/3D projector, annotation editor, and peer exchange.
- **Home Server:** peer ingestion, frame alignment, canonical mapper, voxel integrator, scenegraph storage/publication, and browser 3D visualizer. The mapper accepts observations and scenegraphs from both iPhone and Bracketbot.
- **Bracketbot Runner:** sensor source, localization/frame alignment, scenegraph consumer, and observation publisher. A local mapper and robot action adapter are conditional on the chosen demo behavior.

The purpose is to show that we can use the Annotator to augment the scenegraph with information that the robot can then use for context. We are working towards using the phone to say things like "this is where keys should be stored".

The first milestone is the Home Server and visualizer with two simulated contributors, a canonical voxel map, and a synthetic book detection placed by raycasting. This slice now includes authenticated SDK P2P publication and a read-only scene consumer; see [README.md](README.md) for running it and its current limits. Real device inputs, explicit robot alignment, and integration into the existing robot runtime are next.

The first physical-robot behavior remains to be decided: receiving and interpreting an annotation, navigating to it, or another action.

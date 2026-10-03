# Annotator App

An iPhone AR app peer that creates a scenegraph of Portals, semantic objects, and physical topography, giving the robot spatial context.

## Baseline
- Follow [plan.md](plan.md) and the component-based architecture in [Auki SDK PR #421](https://github.com/aukilabs/auki-sdk/pull/421).
- PR integration points to explore: PortalDetector, Portal mapping, and explicit localization.
- Scope: P2P clusters and scenegraphs only; leave existing portal infrastructure untouched and make no Auki backend calls about portals.
- Auki relay servers may be used for connectivity to peers without an exposed IP address.
- Inject Portal sizes manually; detection and mapping must not trigger backend size lookups.
- Use the supplied size registry in [config/portals.json](config/portals.json). Its dimensions are confirmed encoded-square sizes excluding the white border, suitable for the SDK size field; supplied domain poses are in [config/domain-portals.json](config/domain-portals.json), with stored zero heights preserved rather than interpreted as mounting heights.
- Contribute topography using LiDAR or other ARKit data to a shared, raycastable voxel map that also accepts Bracketbot observations, aligned with Portals and semantic annotations.
- Project detector components' screen-space detections into 3D by raycasting against the voxel map, using the source image's camera calibration and capture-time pose in the map frame.
- Example: a YOLO component detects books on a table; use voxel-map hits to place corresponding book objects in the scenegraph.
- Contribute observations and locally built scenegraphs to the Home Server, which builds and serves the canonical scenegraph.

## Expected components

Proposed responsibilities; SDK reuse and component boundaries remain to be verified. Detector and raycaster placement is provisional.

- **ARKit source:** publish camera images, calibration, capture-time poses, and LiDAR/depth or other available topography observations.
- **Portal detection and localization:** detect Portals using manually supplied sizes and establish alignment between the ARKit session and scenegraph frame.
- **Local mapper:** build local Portal, semantic, and voxel scenegraph content for preview and contribution to the Home Server.
- **Object detector:** produce screen-space detections, such as YOLO book detections, tied to their source image.
- **Voxel raycaster / 3D projector:** combine detections, camera calibration, capture-time pose, and a voxel map to produce 3D object-placement observations.
- **Annotation editor:** create and edit human-authored semantic objects and intended-use labels.
- **Peer exchange:** publish observations and local scenegraphs to the Home Server and receive canonical scenegraph updates through the shared peer runtime.

## To decide
- iOS app stack and ARKit integration.
- Voxel resolution, ARKit inputs, update strategy, and whether non-LiDAR iPhones must be supported.
- Detection-to-ray selection, handling missing or ambiguous hits, and associating repeated detections with existing objects.
- Where detector and raycast components run; YOLO is the motivating example, with the model and integration still to choose.
- First annotation type and placement/editing interaction, including “this is where keys should be stored.”
- Semantic data representation and exchange with the Home Server and Runner.
- How detected payloads and SDK Portal identities map to the supplied local Portal codes; any future size-entry UI.

## First milestone
Display every Portal in Home's scenegraph in AR on the iPhone. Recognize one
known Portal to align the ARKit session with Home's map, then render all Portal
poses and metric outlines without changing Home's data.

The initial native implementation is in [ios/PortalViewer](ios/PortalViewer/README.md).
It connects to Home over authenticated SDK P2P through the Rust mobile bridge,
uses capture-time camera calibration and pose for alignment, and renders all
received Portals. The phone signs in with a User account and retains its own
protected peer identity. Live updates, cancellation and subscription rediscovery
are implemented; physical-device connectivity and AR accuracy still require
verification. LiDAR mapping and semantic annotations are outside this milestone.

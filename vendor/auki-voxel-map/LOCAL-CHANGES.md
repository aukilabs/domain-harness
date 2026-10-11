Based on Auki SDK 3342d589605846f5f45c1de5430dd22923555de2, labs/auki-voxel-map.

Local adapter extension: accept the SDK's AdditiveLinearRgbEvidence grid contract
instead of requiring color_model=None. The SDK accumulator validates color evidence.
The harness adds measured endpoint colors using SDK color decoding and voxelization;
occupancy mapping, clock and source validation remain unchanged. No upstream SDK
or active robot-runner source is modified. Dependencies now target SDK PR #425 at
3342d589605846f5f45c1de5430dd22923555de2; the vendored source origin above is unchanged.

`MAX_SNAPSHOT_BYTES` is raised from 4 MiB to 16 MiB. Colored collaborative maps hit
the former bound near 20,000 occupied voxels and Home then rejected every further
contribution with "snapshot too large". The cell count remains bounded by `MAX_VOXELS`.

# Per-Product subscription demand

Source: auki-sdk 3342d589605846f5f45c1de5430dd22923555de2,
labs/auki-component-protocol. Dependency versions remain pinned to that SDK.

Adds `product_active_subscriptions(product_id)` alongside the existing
endpoint-wide count. Increment only after validating a subscription request;
an RAII guard decrements on disconnect, cancellation, errors or revocation.
Counters live with an export, so re-exporting cannot inherit old demand.
No wire protocol, Catalog, or observation schema changes.

Required so a LiDAR reader does not activate the robot's camera. Remove this
Cargo patch when the pinned SDK provides equivalent per-Product accounting.

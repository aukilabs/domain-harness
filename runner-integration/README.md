# Single Galbot runner integration

The deployed entry point is `src/bin/galbot-runner.rs`, launched through
`scripts/run-galbot-runner.py`. It owns one Auki peer, one component runtime,
sensor publication and local scenegraph mapping. Camera acquisition subprocesses
have no Auki identity or network publisher.

`original/` contains the source adapters inspected from the original Galbot
runner. `src/galbot_sensors.rs`, `scripts/sensor_observations.py` and
`scripts/camera_capture.py` reuse that publication/acquisition implementation.
`retired/` contains the historical mapper-only entry points, excluded from Cargo
binary discovery. Saved map files on the robot are preserved.

The active sensing/mapping deployment is under the original Galbot project at
`/userdata/sam_ws/galbot-g1-runner-portal-mapping/single_runner`. The legacy Flask
control/task server is not started alongside it. No motion or task-control
functionality is enabled by this deployment.

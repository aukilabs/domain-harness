#!/bin/sh
set -eu
bridge_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$bridge_dir/../.." && pwd)
cd "$repo_dir"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
# Device and Apple Silicon simulator. Both slices use the exact same Rust reader.
for rust_target in aarch64-apple-ios aarch64-apple-ios-sim; do
    cargo build --locked -p spatial-portal-bridge --target "$rust_target"
done
output="$bridge_dir/build/SpatialPortalBridge.xcframework"
mkdir -p "$bridge_dir/build"
if [ -d "$output" ]; then rm -rf "$output"; fi
xcodebuild -create-xcframework \
    -library "$repo_dir/target/aarch64-apple-ios/debug/libspatial_portal_bridge.a" -headers "$bridge_dir/include" \
    -library "$repo_dir/target/aarch64-apple-ios-sim/debug/libspatial_portal_bridge.a" -headers "$bridge_dir/include" \
    -output "$output"

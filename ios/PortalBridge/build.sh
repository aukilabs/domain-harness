#!/bin/sh
set -eu
bridge_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$bridge_dir/../.." && pwd)
cd "$repo_dir"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
# Optimized by default: the debug bridge is many times slower at decoding and
# voxel work, which shows up as stale poses and a stuttering map on the phone.
# Use PROFILE=debug for a faster compile while iterating.
profile=${PROFILE:-release}
if [ "$profile" = release ]; then flag=--release; else flag=; fi
# Device and Apple Silicon simulator. Both slices use the exact same Rust reader.
# TARGETS="aarch64-apple-ios" builds the device slice only.
targets=${TARGETS:-"aarch64-apple-ios aarch64-apple-ios-sim"}
libraries=
for rust_target in $targets; do
    cargo build --locked -p spatial-portal-bridge --target "$rust_target" $flag
    libraries="$libraries -library $repo_dir/target/$rust_target/$profile/libspatial_portal_bridge.a -headers $bridge_dir/include"
done
output="$bridge_dir/build/SpatialPortalBridge.xcframework"
mkdir -p "$bridge_dir/build"
if [ -d "$output" ]; then rm -rf "$output"; fi
# shellcheck disable=SC2086
xcodebuild -create-xcframework $libraries -output "$output"

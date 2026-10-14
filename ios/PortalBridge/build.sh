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
# The commit the bridge is built from, marked dirty when the Rust it compiles has
# uncommitted changes. Stamped into the library; the app logs it at start.
source=$(git rev-parse HEAD)
if [ -n "$(git status --porcelain -- src ios/PortalBridge Cargo.toml Cargo.lock vendor)" ]; then source="$source-dirty"; fi
export SPATIAL_BRIDGE_SOURCE="$source"
# Package from where cargo actually builds, which CARGO_TARGET_DIR or a cargo
# config can move away from ./target.
target_dir=$(cargo metadata --format-version 1 --no-deps --locked | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
[ -n "$target_dir" ] || { echo "build.sh: cargo did not report its target directory" >&2; exit 1; }
libraries=
checksums=
for rust_target in $targets; do
    cargo build --locked -p spatial-portal-bridge --target "$rust_target" $flag
    library="$target_dir/$rust_target/$profile/libspatial_portal_bridge.a"
    libraries="$libraries -library $library -headers $bridge_dir/include"
    checksums="$checksums$rust_target $(shasum -a 256 "$library" | cut -d' ' -f1)
"
done
output="$bridge_dir/build/SpatialPortalBridge.xcframework"
mkdir -p "$bridge_dir/build"
if [ -d "$output" ]; then rm -rf "$output"; fi
# shellcheck disable=SC2086
xcodebuild -create-xcframework $libraries -output "$output"
# What this framework is, for checking an installed app against the source.
printf 'source %s\nprofile %s\n%s' "$source" "$profile" "$checksums" > "$bridge_dir/build/SpatialPortalBridge.source"
cat "$bridge_dir/build/SpatialPortalBridge.source"

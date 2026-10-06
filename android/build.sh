#!/bin/sh
set -eu
android_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$android_dir/.." && pwd)
sdk_dir=${ANDROID_HOME:-"$repo_dir/.deps/android-toolchain/sdk"}
core_dir=${SPATIAL_CORE_DIR:-"$repo_dir"}
export ANDROID_HOME="$sdk_dir"
ndk_dir="$sdk_dir/ndk/27.2.12479018"
toolchain="$ndk_dir/toolchains/llvm/prebuilt/darwin-x86_64/bin"
test -x "$toolchain/aarch64-linux-android28-clang" || { echo 'Install Android NDK 27.2.12479018 first.' >&2; exit 1; }
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/aarch64-linux-android28-clang"
export CC_aarch64_linux_android="$toolchain/aarch64-linux-android28-clang"
export AR_aarch64_linux_android="$toolchain/llvm-ar"
export CXX_aarch64_linux_android="$toolchain/aarch64-linux-android28-clang++"
cd "$repo_dir"
cargo build --locked --manifest-path "$core_dir/Cargo.toml" --target-dir "$repo_dir/target" -p spatial-portal-bridge --target aarch64-linux-android
mkdir -p "$android_dir/native/arm64-v8a"
cp target/aarch64-linux-android/debug/libspatial_portal_bridge.a "$android_dir/native/arm64-v8a/"
cd "$android_dir"
./gradlew assembleDebug lintDebug testDebugUnitTest

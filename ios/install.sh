#!/bin/sh
# Build the Rust bridge and the iPhone app from this checkout, together, and install
# the app on a phone. Xcode links whatever bridge was built last and never rebuilds
# it, so building the app alone can ship old Rust.
#
#   sh ios/install.sh <device id>      # from `xcrun devicectl list devices`
#
# Installing over the app keeps its data: it is never uninstalled first.
# DERIVED_DATA sets where Xcode builds (default: ios/build).
set -eu
device=${1:?usage: sh ios/install.sh <device id>}
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_dir"
derived=${DERIVED_DATA:-$repo_dir/ios/build}
TARGETS=aarch64-apple-ios sh ios/PortalBridge/build.sh
xcodebuild -project ios/PortalViewer/PortalViewer.xcodeproj -scheme PortalViewer -configuration Release \
    -destination 'generic/platform=iOS' -derivedDataPath "$derived" -allowProvisioningUpdates build
app=$(ls -d "$derived"/Build/Products/Release-iphoneos/*.app | head -1)
xcrun devicectl device install app --device "$device" "$app"
echo "Installed $(basename "$app") built with:"
cat ios/PortalBridge/build/SpatialPortalBridge.source

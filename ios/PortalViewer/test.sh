#!/bin/sh
set -eu
cd "$(dirname "$0")"
test_dir=$(mktemp -d /tmp/portal-viewer-test.XXXXXX)
trap 'rm -rf "$test_dir"' EXIT
xcrun swiftc -module-cache-path "$test_dir/cache" App/PortalMap.swift App/PortalPose.swift App/SemanticVolumeModel.swift App/GeoReference.swift Tests/main.swift -o "$test_dir/tests"
"$test_dir/tests"

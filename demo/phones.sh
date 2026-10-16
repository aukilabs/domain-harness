#!/bin/sh
# Demo: build the iPhone app, install it and launch it signed in on the demo phones.
#
#   demo/phones.sh [--build] [scene-id] [home-url]
#
#   --build    build the Rust bridge and the app, and install them first
#   scene-id   a scene on Home; by default whatever scene Home offers now, so
#              this works straight after scenegraphs are deleted and recreated
#   home-url   defaults to Nils's Home, http://192.168.1.26:4329
#
#   PHONES="staples spaceship"  which phones (default both)
#   TEAM / BUNDLE               signing team and bundle ID for the build
#
# The demo login is in demo/demo-login.env. Each phone's console goes to
# demo/logs/<phone>.log; `tail -f` it to follow along.
set -eu
cd "$(dirname "$0")/.."

build=0
if [ "${1:-}" = "--build" ]; then build=1; shift; fi
scene=${1:-}
home=${2:-http://192.168.1.26:4329}
phones=${PHONES:-staples spaceship}
team=${TEAM:-V7LQ934WJD}
bundle=${BUNDLE:-com.luckybulldozer.auki.PortalViewer}

device_id() {
  case "$1" in
    staples) echo 00008150-001814123A30C01C ;;
    spaceship) echo 00008130-000A25323CB8001C ;;
    officemax) echo 00008110-000A7CC21484801E ;;
    *) echo "Unknown phone $1" >&2; exit 1 ;;
  esac
}

if [ "$build" = 1 ]; then
  # Homebrew's rustc is too old for this repo; use rustup's stable toolchain.
  PATH="$(rustup run stable rustc --print sysroot)/bin:$PATH"; export PATH
  CARGO_INCREMENTAL=0 TARGETS=aarch64-apple-ios sh ios/PortalBridge/build.sh
  derived=${DERIVED_DATA:-build/demo-ios}
  xcodebuild -project ios/PortalViewer/PortalViewer.xcodeproj -scheme PortalViewer \
    -configuration Release -destination generic/platform=iOS -derivedDataPath "$derived" \
    DEVELOPMENT_TEAM="$team" PRODUCT_BUNDLE_IDENTIFIER="$bundle" CODE_SIGN_STYLE=Automatic \
    -allowProvisioningUpdates -quiet build
  app="$derived/Build/Products/Release-iphoneos/PortalViewer.app"
  # One phone at a time: parallel installs fail on one of them.
  for phone in $phones; do
    echo "Installing on $phone"
    xcrun devicectl device install app --device "$(device_id "$phone")" "$app" >/dev/null
  done
fi

# Always a fresh profile: a recreated scenegraph has a new epoch in its frame, and the
# profile a phone saved earlier would point at the old one.
if [ -n "$scene" ]; then
  profile=$(curl -s -f -m 5 "$home/api/local-scenes/$scene/connection") \
    || { echo "Home at $home has no scene $scene, or is not ready"; exit 1; }
else
  profile=$(curl -s -f -m 5 "$home/api/home-connection") \
    || { echo "Home at $home is not ready"; exit 1; }
fi
echo "Pairing to $(echo "$profile" | python3 -c 'import json,sys;print(json.load(sys.stdin)["frame"])')"
set -a; . demo/demo-login.env; set +a
env_json=$(AUKI_HOME_PROFILE="$profile" AUKI_HOME_URL="$home" python3 -c 'import json,os;print(json.dumps({k:os.environ[k] for k in ("AUKI_HOME_PROFILE","AUKI_HOME_URL","AUKI_EMAIL","AUKI_PASSWORD") if os.environ.get(k)}))')

mkdir -p demo/logs
for phone in $phones; do
  name=$(echo "$phone" | awk '{print toupper(substr($0,1,1)) substr($0,2)}')
  xcrun devicectl device process launch --device "$(device_id "$phone")" --terminate-existing \
    --console --environment-variables "$env_json" "$bundle" -phoneName "$name" \
    > "demo/logs/$phone.log" 2>&1 &
  echo "Launching on $phone (log: demo/logs/$phone.log)"
done
# A locked phone refuses the launch within a few seconds.
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  for phone in $phones; do
    if grep -q "Locked" "demo/logs/$phone.log" 2>/dev/null && ! grep -q "reported" "demo/logs/$phone.log.state" 2>/dev/null; then
      echo "$phone: phone locked"; echo reported > "demo/logs/$phone.log.state"
    fi
  done
done
rm -f demo/logs/*.log.state
wait

#!/usr/bin/env bash
# Fail unless every version field equals the given tag (vX.Y.Z or X.Y.Z). CI runs this on tag pushes.
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?usage: scripts/check-version.sh vX.Y.Z}"; V="${V#v}"
M=modules/expo-cardboard-compendium
bad=0
chk() { if [ "$2" != "$V" ]; then echo "version mismatch: $1 is '$2', tag is '$V'" >&2; bad=1; fi; }
chk Cargo.toml "$(sed -nE '0,/^version = "([^"]*)"/s//\1/p' Cargo.toml | head -1)"
chk "$M/package.json" "$(python3 -c "import json;print(json.load(open('$M/package.json'))['version'])")"
chk "$M/android/build.gradle version" "$(sed -nE "s/^version = '([^']*)'/\1/p" $M/android/build.gradle)"
chk "$M/android/build.gradle versionName" "$(sed -nE 's/.*versionName "([^"]*)".*/\1/p' $M/android/build.gradle)"
chk "$M podspec" "$(sed -nE "s/.*s\.version +=  *'([^']*)'.*/\1/p" $M/ios/ExpoCardboardCompendium.podspec)"
[ $bad = 0 ] && echo "versions all $V"
exit $bad

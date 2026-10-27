#!/usr/bin/env bash
# Set the version everywhere it is written, so it matches the release tag (vX.Y.Z):
#   scripts/set-version.sh 0.0.3
# Files: Cargo.toml ([workspace.package]; Cargo.lock refreshed), the module's package.json,
# android/build.gradle (version + versionName), the podspec, and the fetch scripts' User-Agent.
# scripts/check-version.sh verifies they agree (CI runs it on tags).
set -euo pipefail
cd "$(dirname "$0")/.."
V="${1:?usage: scripts/set-version.sh X.Y.Z}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z (no leading v)" >&2; exit 1; }
M=modules/expo-cardboard-compendium
sed -i -E "0,/^version = \"[^\"]*\"/s//version = \"$V\"/" Cargo.toml
sed -i -E "s/^(version = )'[^']*'/\1'$V'/; s/(versionName )\"[^\"]*\"/\1\"$V\"/" $M/android/build.gradle
sed -i -E "s/(s\.version +=  *)'[^']*'/\1'$V'/" $M/ios/ExpoCardboardCompendium.podspec
sed -i -E "s#expo-cardboard-compendium/[0-9]+\.[0-9]+\.[0-9]+#expo-cardboard-compendium/$V#" scripts/fetch-bulk.sh scripts/daily-prices.sh
python3 - "$V" "$M/package.json" <<'PY'
import json, sys
v, p = sys.argv[1], sys.argv[2]
d = json.load(open(p)); d["version"] = v
json.dump(d, open(p, "w"), indent=2); open(p, "a").write("\n")
PY
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null
scripts/check-version.sh "v$V"
echo "now: commit, then tag v$V and push the tag"

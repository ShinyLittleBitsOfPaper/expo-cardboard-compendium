#!/usr/bin/env bash
# Offline test of scripts/changelog.py and the changelog part of cut-release.sh (local bare remote, stubbed cargo).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
git init -q --bare "$t/remote.git"
git clone -q "$t/remote.git" "$t/w" 2>/dev/null; cd "$t/w"
git config user.email t@t; git config user.name t; git checkout -q -b trunk
mkdir -p scripts modules/expo-cardboard-compendium/android modules/expo-cardboard-compendium/ios; cp "$here"/scripts/{changelog.py,cut-release.sh,set-version.sh,check-version.sh,fetch-bulk.sh,daily-prices.sh} scripts/
M=modules/expo-cardboard-compendium
printf '[workspace.package]\nversion = "0.1.0"\n' > Cargo.toml
echo '{"version":"0.1.0"}' > $M/package.json
printf "version = '0.1.0'\ndefaultConfig { versionName \"0.1.0\" }\n" > $M/android/build.gradle
printf "  s.version      = '0.1.0'\n" > $M/ios/ExpoCardboardCompendium.podspec
mkdir bin; echo bin/ > .gitignore; printf '#!/bin/sh\nexit 0\n' > bin/cargo; chmod +x bin/cargo; export PATH="$PWD/bin:$PATH"
cat > CHANGELOG.md <<'EOF'
# Changelog

## Unreleased

### Fixed
- 2026-01-02, 0.1.0-2: Plain entry without bold. Second sentence (e.g. `a. b`) is dropped.

### Added
- 2026-01-01, 0.1.0-1: **Bold `summary` here.** Details
  continue on a second line.

## 0.1.0 - 2025-12-31

### Added
- 2025-12-31, 0.0.9-1: **Old.** Old.
EOF
git add -A; git commit -qm init; git push -q origin trunk 2>/dev/null
bash scripts/cut-release.sh patch --dry-run | tee "$t/out.txt" >/dev/null
grep -q '^- 2026-01-02, 0.1.0-2: Plain entry without bold$' "$t/out.txt"
grep -q '^- 2026-01-01, 0.1.0-1: Bold summary here$' "$t/out.txt"
python3 scripts/changelog.py check 0.1.0-2 2>/dev/null
! scripts/changelog.py check 0.1.0-9 2>/dev/null
bash scripts/cut-release.sh patch >/dev/null
git log -1 --format=%B | grep -q '^- 2026-01-01, 0.1.0-1: Bold summary here$'
git tag -l -n9 v0.1.1 | grep -q '0.1.0-2: Plain entry'
grep -q '^## 0.1.1 - ' CHANGELOG.md
grep -q '^## 0.1.0 - 2025-12-31' CHANGELOG.md
[ -z "$(git status --porcelain)" ]
git ls-remote --tags origin | grep -q v0.1.1
! scripts/cut-release.sh patch 2>/dev/null   # nothing under Unreleased
echo "changelog tests passed"

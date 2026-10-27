#!/usr/bin/env bash
# Milestone 1, phase B: publish a stage directory made by data-build.sh to GitHub Releases.
#
#   scripts/data-publish.sh [stage_dir]        (default data/stage)
#
# Order matters so a client never sees a manifest that points at a missing file:
#   1. draft release data-<run> with this run's patches        4. publish the draft
#   2. upload the new full files to data-latest (--clobber)    5. upload manifest.json LAST
#   3. verify uploaded sizes                                   6. delete old full files, prune old data-* releases
# Env: PUBLISH=false (dry run: nothing is touched), GH (gh binary, default gh), GITHUB_REPOSITORY, RETAIN_DAYS (30),
#      PUBLISH_NOW (testing: pretend this is now), GH_TOKEN (gh reads it). data releases are always prerelease and never "Latest" (code releases keep that label).
set -euo pipefail
cd "$(dirname "$0")/.."
STAGE="${1:-data/stage}"; GH="${GH:-gh}"; REPO="${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY}"
RETAIN_DAYS="${RETAIN_DAYS:-30}"; M="$STAGE/manifest.json"
[ -f "$STAGE/NOTHING" ] && { echo "nothing to publish"; exit 0; }
[ -f "$M" ] || { echo "$M missing: run scripts/data-build.sh first" >&2; exit 1; }
RUN="$(jq -r .run "$M")"
read -r -a LATEST <<<"$(sed -n 's/^latest: //p' "$STAGE/assets.txt")"
read -r -a RUNFILES <<<"$(sed -n 's/^run: *//p' "$STAGE/assets.txt")"
echo "run $RUN: ${#LATEST[@]} full files, ${#RUNFILES[@]} patch files"
if [ "${PUBLISH:-true}" != true ]; then echo "dry run (PUBLISH=$PUBLISH): not publishing"; exit 0; fi

stage_paths() { local p=(); for f in "$@"; do p+=("$STAGE/$f"); done; printf '%s\n' "${p[@]}"; }
NOTES="Data release ($RUN): patches from the previous run. Not a code release; clients read data-latest/manifest.json."

# 1. immutable per-run release (draft until its files are up)
if [ "${#RUNFILES[@]}" -gt 0 ]; then
  mapfile -t RP < <(stage_paths "${RUNFILES[@]}")
  $GH release create "$RUN" --repo "$REPO" --draft --prerelease --latest=false --title "data $RUN" --notes "$NOTES" "${RP[@]}"
fi
# 2. rolling release with the current full files
$GH release view data-latest --repo "$REPO" >/dev/null 2>&1 || \
  $GH release create data-latest --repo "$REPO" --prerelease --latest=false --title "data-latest (rolling)" \
    --notes "Rolling data release: manifest.json and the current full files. Not a code release."
mapfile -t LP < <(stage_paths "${LATEST[@]}")
$GH release upload data-latest --repo "$REPO" --clobber "${LP[@]}"
# 3. verify what landed
ASSETS="$($GH release view data-latest --repo "$REPO" --json assets | jq -r '.assets[] | "\(.name) \(.size)"')"
for f in "${LATEST[@]}"; do
  want="$(stat -c %s "$STAGE/$f")"; got="$(printf '%s\n' "$ASSETS" | awk -v n="$f" '$1==n{print $2}')"
  [ "$want" = "$got" ] || { echo "upload check failed for $f: want $want got ${got:-missing}" >&2; exit 1; }
done
# 4. publish the draft (still prerelease, never Latest)
[ "${#RUNFILES[@]}" -gt 0 ] && $GH release edit "$RUN" --repo "$REPO" --draft=false --prerelease --latest=false
# 5. manifest last
$GH release upload data-latest --repo "$REPO" --clobber "$M"
# 6a. old full files out
printf '%s\n' "$ASSETS" | awk '{print $1}' | while read -r n; do
  [ "$n" = manifest.json ] && continue
  keep=0; for f in "${LATEST[@]}"; do [ "$f" = "$n" ] && keep=1; done
  [ "$keep" = 1 ] || $GH release delete-asset data-latest "$n" --repo "$REPO" --yes
done
# 6b. prune expired run releases (one day of slack over the manifest's chain window; never one the manifest still uses)
CUT="$(date -u -d "${PUBLISH_NOW:-now} - $((RETAIN_DAYS + 1)) days" +%Y-%m-%dT%H:%M:%SZ)"
$GH release list --repo "$REPO" --limit 300 --json tagName,createdAt | \
  jq -r --arg cut "$CUT" '.[] | select(.tagName | test("^data-[0-9]")) | select(.createdAt < $cut) | .tagName' | \
while read -r t; do
  if grep -q "/$t/" "$M"; then echo "keeping $t (still referenced)"; else $GH release delete "$t" --repo "$REPO" --cleanup-tag --yes; fi
done
echo "published $RUN; manifest: https://github.com/$REPO/releases/download/data-latest/manifest.json"

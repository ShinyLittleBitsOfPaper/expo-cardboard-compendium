#!/usr/bin/env bash
# Art-hash overlay step of a data run (called by data-build.sh; also runnable alone for a dry check).
#   scripts/data-art.sh <stage_dir> <cards.coqd> <cards_stamp> [prev_manifest.json]
# Reads the hash cache from the persistent `art-hash-cache` release (assets art-hashes.tsv.zst), rebuilds the `arthash` overlay for this
# card file from the bulk file WITHOUT downloading any image, and writes `<stage>/art.spec` = the `--overlay` spec for `coqc manifest`
# (empty when there is no overlay). Patches against the previously published overlay when it can (same epoch, `coqc patch` ok).
# Never silently drops a published overlay: if the previous manifest has `arthash` and the cache cannot be fetched, it fails.
# Env: GITHUB_REPOSITORY, ART_CACHE_URL and ART_WORK (override, testing), BULK (default data/daily/raw/default-cards.jsonl), BIN.
set -euo pipefail
cd "$(dirname "$0")/.."
STAGE="${1:?stage dir}"; CARDS="${2:?cards.coqd}"; CS="${3:?cards stamp}"; PREVM="${4:-}"
BIN="${BIN:-./target/release/coqc}"; BULK="${BULK:-data/daily/raw/default-cards.jsonl}"
REPO="${GITHUB_REPOSITORY:-}"
CACHE_URL="${ART_CACHE_URL:-https://github.com/$REPO/releases/download/art-hash-cache/art-hashes.tsv.zst}"
W="${ART_WORK:-data/art}"; mkdir -p "$W/releases"; : > "$STAGE/art.spec"
ds() { [ -n "$PREVM" ] && [ -f "$PREVM" ] && jq -r --arg id arthash ".datasets[] | select(.id == \$id) | $1" "$PREVM" || true; }
PART="$(ds .stamp)"
echo "== art hash overlay"
if curl -fsSL --retry 3 -o "$W/cache.tsv.zst" "$CACHE_URL"; then
  "$BIN" unzst "$W/cache.tsv.zst" -o "$W/cache.tsv" >/dev/null
  NEW="$STAGE/arthash-$CS.coqd"
  "$BIN" art-hash "$BULK" --cards "$CARDS" --cache "$W/cache.tsv" -o "$NEW" --todo "$STAGE/art-todo.tsv" | tee "$STAGE/art-report.txt"
  SPEC="arthash=$NEW,of=cards"
  if [ -n "$PART" ] && [ "$PART" != "$CS" ]; then
    PREV="$W/releases/arthash-$PART.coqd"
    [ -f "$PREV" ] || { curl -fsSL --retry 3 -o "$PREV" "$(ds .file.url)" && [ "$(sha256sum "$PREV" | cut -d' ' -f1)" = "$(ds .file.sha256)" ] || rm -f "$PREV"; }
    P="$STAGE/arthash-$PART-to-$CS.coqp"
    if [ -f "$PREV" ] && "$BIN" patch "$PREV" "$NEW" -o "$P" >/dev/null 2>&1 \
       && "$BIN" apply "$PREV" "$P" -o "$W/verify.coqd" >/dev/null 2>&1 && cmp -s "$W/verify.coqd" "$NEW"; then
      SPEC+=",patch=$P"; echo "patch $(basename "$P"): $(stat -c %s "$P") bytes (replay verified)"
    else
      rm -f "$P"; echo "no usable patch from $PART (new epoch or unavailable): clients download the full overlay"
    fi
  fi
  cp "$NEW" "$W/releases/"
  echo "$SPEC" > "$STAGE/art.spec"
  echo "illustrations still without a hash: $(wc -l < "$STAGE/art-todo.tsv") (the cron cannot fetch images; run scripts/art-hash.sh update locally, then release)"
elif [ -n "$PART" ]; then
  echo "art hash cache not fetchable ($CACHE_URL) but arthash is already published: refusing to drop it" >&2; exit 1
else
  echo "no art-hash-cache release yet: no arthash overlay in this run"
fi

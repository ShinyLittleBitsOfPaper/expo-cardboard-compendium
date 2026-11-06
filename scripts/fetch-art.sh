#!/usr/bin/env bash
# Download the art crops listed by `coqc art-hash ... --todo todo.tsv` (illustration_id<TAB>url), named <illustration_id>.jpg.
# Usage: scripts/fetch-art.sh todo.tsv OUT_DIR [PARALLEL=4]
# Resumable (skips files that exist), polite (descriptive User-Agent, small parallelism, a short pause per request).
# Run it from your machine; Scryfall's permission reply is still pending for any automated/cron use.
set -euo pipefail
LIST="${1:?usage: fetch-art.sh todo.tsv OUT_DIR [PARALLEL]}"; OUT="${2:?}"; PAR="${3:-4}"
UA="expo-cardboard-compendium-art-hash/0.0.3 (github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium)"
mkdir -p "$OUT"
export OUT UA
fetch_one() {
  id="$1"; url="$2"; f="$OUT/$id.jpg"
  [ -s "$f" ] && return 0
  curl -fsS --retry 3 --retry-delay 2 -A "$UA" -o "$f.part" "$url" && mv "$f.part" "$f" || { rm -f "$f.part"; echo "fail $id" >&2; }
  sleep 0.1
}
export -f fetch_one
tr '\t' ' ' < "$LIST" | xargs -P "$PAR" -L 1 bash -c 'fetch_one "$0" "$1"'
echo "have $(ls "$OUT" | grep -c '\.jpg$') images in $OUT"

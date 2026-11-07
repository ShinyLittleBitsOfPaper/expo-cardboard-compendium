#!/usr/bin/env bash
# Download the art crops listed by `coqc art-hash ... --todo todo.tsv` (illustration_id<TAB>url), named <illustration_id>.jpg.
# Usage: scripts/fetch-art.sh todo.tsv OUT_DIR [PARALLEL=16] [BATCH=1000]
# Uses curl's own parallel mode (connection reuse + many transfers at once): a single request can take many seconds on a
# slow link, so per-file curl with a few workers cannot finish ~60k images. Resumable (existing files are skipped; rerun
# after any interruption), polite (descriptive User-Agent), one progress line per batch. Needs curl >= 7.66.
# Run it from your machine; Scryfall's permission reply is still pending for any automated/cron use.
set -euo pipefail
LIST="${1:?usage: fetch-art.sh todo.tsv OUT_DIR [PARALLEL] [BATCH]}"; OUT="${2:?}"; PAR="${3:-16}"; BATCH="${4:-1000}"
UA="expo-cardboard-compendium-art-hash/0.0.3 (github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium)"
mkdir -p "$OUT"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
EXTRA=(); curl --help all 2>/dev/null | grep -q -- '--remove-on-error' && EXTRA+=(--remove-on-error)
# drop empty leftovers from an interrupted run, then list what is still missing
find "$OUT" -name '*.jpg' -size 0 -delete
while IFS=$'\t' read -r id url; do [ -s "$OUT/$id.jpg" ] || printf '%s\t%s\n' "$id" "$url"; done < "$LIST" > "$TMP/pending"
TOTAL=$(wc -l < "$TMP/pending")
echo "to fetch: $TOTAL (parallel $PAR, batches of $BATCH)"
[ "$TOTAL" -eq 0 ] && exit 0
split -l "$BATCH" -d -a 5 "$TMP/pending" "$TMP/b."
DONE=0
for b in "$TMP"/b.?????; do
  awk -F'\t' -v out="$OUT" '{printf "url = \"%s\"\noutput = \"%s/%s.jpg\"\n", $2, out, $1}' "$b" > "$b.cfg"
  curl -sS --fail --parallel --parallel-max "$PAR" --connect-timeout 20 --max-time 180 --retry 3 --retry-delay 2 \
       "${EXTRA[@]}" -A "$UA" -K "$b.cfg" || echo "some downloads in this batch failed (rerun to retry them)" >&2
  DONE=$((DONE + $(wc -l < "$b")))
  echo "$(date +%T) $DONE/$TOTAL requested, $(find "$OUT" -name '*.jpg' -size +0 | wc -l) images on disk"
done
echo "done; rerun once more to retry any failures (a rerun with nothing missing prints 'to fetch: 0')"

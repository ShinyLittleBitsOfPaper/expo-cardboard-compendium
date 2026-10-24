#!/usr/bin/env bash
# One day of the pipeline: fetch -> compile (stable rows via --prev) -> patch -> verify the patch.
#
#   scripts/daily.sh [workdir] [--force]        (default workdir: data/daily)
#
# Layout: <workdir>/raw/default-cards.jsonl, <workdir>/releases/cards-YYYYMMDD.coqd,
#         <workdir>/patches/<prev>-to-<new>.coqp
# Run it once a day. The first run makes the first release; later runs build on the latest one.
# Prints a summary of each compile (counts, derived URLs, healed printings, sizes); run `coqc compile`
# directly for the full report.
# Env: COQC_STAMP=YYYYMMDD overrides the date label (testing), plus the fetch script's variables.
set -euo pipefail
cd "$(dirname "$0")/.."

WORK="data/daily"; FETCH_ARGS=()
for a in "$@"; do
  case "$a" in
    --force) FETCH_ARGS+=(--force) ;;
    *) WORK="$a" ;;
  esac
done
BIN=./target/release/coqc
SUMMARY='^(skipped|oracle cards|printings|images|derived key|healed printings|epoch|time:)|^    (oracle|printing|oracle_ranks|meta) '
[ -x "$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }
mkdir -p "$WORK/raw" "$WORK/releases" "$WORK/patches"

scripts/fetch-bulk.sh "$WORK/raw" ${FETCH_ARGS[@]+"${FETCH_ARGS[@]}"}
RAW="$WORK/raw/default-cards.jsonl"
STAMP="${COQC_STAMP:-$(date -u -r "$RAW" +%Y%m%d)}"
NEW="$WORK/releases/cards-$STAMP.coqd"
if [ -f "$NEW" ]; then echo "$NEW already exists (nothing new since $STAMP)"; exit 0; fi

PREV="$(ls -1 "$WORK"/releases/cards-*.coqd 2>/dev/null | sort | tail -1 || true)"
if [ -n "$PREV" ]; then
  echo "== compiling $STAMP on top of $(basename "$PREV")"
  "$BIN" compile "$RAW" -o "$NEW" --prev "$PREV" --source-date "$STAMP" --progress | grep -E "$SUMMARY"
  PSTAMP="$(basename "$PREV" .coqd)"; PSTAMP="${PSTAMP#cards-}"
  PATCH="$WORK/patches/$PSTAMP-to-$STAMP.coqp"
  "$BIN" patch "$PREV" "$NEW" -o "$PATCH"
  TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT
  "$BIN" apply "$PREV" "$PATCH" -o "$TMP" | tail -1
  cmp "$TMP" "$NEW" && echo "patch verified: applying it to $(basename "$PREV") reproduces $(basename "$NEW") exactly"
else
  echo "== first release: compiling $STAMP"
  "$BIN" compile "$RAW" -o "$NEW" --source-date "$STAMP" --progress | grep -E "$SUMMARY"
fi
echo "release: $NEW ($(du -h "$NEW" | cut -f1))"

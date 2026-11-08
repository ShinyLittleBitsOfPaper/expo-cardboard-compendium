#!/usr/bin/env bash
# Local art-hash maintenance (docs/phash-design.md). Hashes live in a cache TSV that the daily data workflow reads from the
# persistent `art-hash-cache` release; the workflow rebuilds the `arthash` overlay from it every run but CANNOT fetch images,
# so a person runs `update` + `release` now and then (new sets bring new illustrations).
#
#   scripts/art-hash.sh status     how many illustrations have a hash / are still missing (no downloads)
#   scripts/art-hash.sh update     fetch the missing art crops, hash them, rewrite the cache and a local overlay
#   scripts/art-hash.sh release    upload the cache to the `art-hash-cache` release (creates it on first use)
#     options: --dry-run (release: build and check the upload, touch nothing)  --no-fetch (update: hash what is on disk only)
#              --force (release: skip the "cache looks too small" guard)
# Env: CARDS (card file; default newest data/daily/releases/cards-*.coqd), BULK (default data/daily/raw/default-cards.jsonl),
#      ART_DIR (image folder, default ../card-art), CACHE (default data/art/art-hashes.tsv), PARALLEL (download parallelism, 8),
#      GITHUB_REPOSITORY (default ShinyLittleBitsOfPaper/expo-cardboard-compendium), GH (gh binary).
# Prerequisites: a built target/release/coqc, curl, and for `release` an authenticated gh. Get the card file and bulk file with scripts/daily.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
CMD="${1:-}"; shift || true
DRY=0; NOFETCH=0; FORCE=0
for a in "$@"; do case "$a" in --dry-run) DRY=1;; --no-fetch) NOFETCH=1;; --force) FORCE=1;; *) echo "unknown option $a" >&2; exit 2;; esac; done
BIN=./target/release/coqc; GH="${GH:-gh}"; REPO="${GITHUB_REPOSITORY:-ShinyLittleBitsOfPaper/expo-cardboard-compendium}"
BULK="${BULK:-data/daily/raw/default-cards.jsonl}"; ART_DIR="${ART_DIR:-../card-art}"; CACHE="${CACHE:-data/art/art-hashes.tsv}"
CARDS="${CARDS:-$(ls -1 data/daily/releases/cards-*.coqd 2>/dev/null | sort | tail -1 || true)}"
[ -x "$BIN" ] || { echo "build first: cargo build --release -p coqc" >&2; exit 1; }
mkdir -p "$(dirname "$CACHE")" data/art
# the first runs kept the cache next to the card releases; pick it up once
[ -f "$CACHE" ] || { [ -f data/daily/releases/art-hashes.tsv ] && cp data/daily/releases/art-hashes.tsv "$CACHE" && echo "copied the existing cache to $CACHE"; } || true

need_inputs() {
  [ -n "$CARDS" ] && [ -f "$CARDS" ] || { echo "no card file: run scripts/daily.sh (or set CARDS)" >&2; exit 1; }
  [ -f "$BULK" ] || { echo "no bulk file $BULK: run scripts/daily.sh (or set BULK)" >&2; exit 1; }
}
run_hash() {  # [extra args] ; writes the overlay + todo list, prints the report
  local stamp; stamp="$(basename "$CARDS" .coqd)"; stamp="${stamp#cards-}"
  "$BIN" art-hash "$BULK" --cards "$CARDS" --cache "$CACHE" -o "data/art/arthash-$stamp.coqd" --todo data/art/todo.tsv "$@"
}

case "$CMD" in
  status)
    need_inputs
    run_hash
    echo "cache: $CACHE ($(wc -l < "$CACHE") hashes); still missing: $(wc -l < data/art/todo.tsv) (images dir $ART_DIR has $(ls "$ART_DIR" 2>/dev/null | grep -c '\.jpg$' || true))"
    ;;
  update)
    need_inputs
    run_hash >/dev/null
    N="$(wc -l < data/art/todo.tsv)"
    echo "$N illustrations without a hash"
    if [ "$NOFETCH" = 0 ] && [ "$N" -gt 0 ]; then
      scripts/fetch-art.sh data/art/todo.tsv "$ART_DIR" "${PARALLEL:-8}"
    fi
    run_hash --images "$ART_DIR"
    echo "next: scripts/art-hash.sh release   (uploads $CACHE; the next data run publishes the new overlay)"
    ;;
  release)
    [ -f "$CACHE" ] || { echo "no cache at $CACHE: run scripts/art-hash.sh update first" >&2; exit 1; }
    N="$(wc -l < "$CACHE")"
    [ "$N" -ge 1000 ] || [ "$FORCE" = 1 ] || { echo "cache has only $N hashes (expected tens of thousands); --force to upload anyway" >&2; exit 1; }
    Z="$(mktemp -d)/art-hashes.tsv.zst"
    "$BIN" zst "$CACHE" -o "$Z" --level 19
    if [ "$DRY" = 1 ]; then echo "dry run: would upload $N hashes ($(stat -c %s "$Z") bytes) to release art-hash-cache of $REPO"; exit 0; fi
    $GH release view art-hash-cache --repo "$REPO" >/dev/null 2>&1 || \
      $GH release create art-hash-cache --repo "$REPO" --prerelease --latest=false --title "art hash cache" \
        --notes "Persistent hash cache read by the data workflow (never pruned). Not a code or data release."
    $GH release upload art-hash-cache --repo "$REPO" --clobber "$Z"
    GOT="$($GH release view art-hash-cache --repo "$REPO" --json assets | jq -r '.assets[] | select(.name == "art-hashes.tsv.zst") | .size')"
    [ "$GOT" = "$(stat -c %s "$Z")" ] || { echo "upload check failed: want $(stat -c %s "$Z") got ${GOT:-missing}" >&2; exit 1; }
    echo "uploaded $N hashes: https://github.com/$REPO/releases/download/art-hash-cache/art-hashes.tsv.zst"
    ;;
  *) sed -n 2,16p "$0"; exit 2;;
esac

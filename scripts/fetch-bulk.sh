#!/usr/bin/env bash
# Download Scryfall bulk data (default: default_cards) into ./data as plain JSONL.
#   scripts/fetch-bulk.sh [dest_dir] [--force] [--type default_cards]
# Follows Scryfall's API etiquette: identifying User-Agent, Accept header, and
# no re-download within 24h (bulk files only refresh every 12-24h anyway).
# Env: COQC_USER_AGENT (please include a contact), COQC_BULK_META_URL (testing).
set -euo pipefail

DEST="data"; TYPE="default_cards"; FORCE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --force) FORCE=1 ;;
    --type) TYPE="$2"; shift ;;
    *) DEST="$1" ;;
  esac
  shift
done
UA="${COQC_USER_AGENT:-expo-scryfall-engine/0.0.1 (set COQC_USER_AGENT to include a contact)}"
META_URL="${COQC_BULK_META_URL:-https://api.scryfall.com/bulk-data/$TYPE}"
mkdir -p "$DEST"

meta="$(curl -fsS -H "User-Agent: $UA" -H "Accept: application/json;q=0.9,*/*;q=0.8" "$META_URL")"
# Prints the first string value for a JSON key; prints nothing (and succeeds) if absent.
field() { printf '%s' "$meta" | grep -o "\"$1\" *: *\"[^\"]*\"" | head -1 | sed -E 's/^[^:]*: *"(.*)"$/\1/' || true; }

uri="$(field jsonl_download_uri)"
[ -n "$uri" ] || uri="$(field download_uri)"   # older API shape: a single JSON array
updated="$(field updated_at)"
if [ -z "$uri" ]; then
  echo "no jsonl_download_uri/download_uri in the bulk-data response:" >&2
  echo "$meta" | head -c 600 >&2; echo >&2
  exit 1
fi

base="${uri%%\?*}"; name="${base##*/}"        # e.g. default-cards-20261022210550.jsonl.gz
plain="${name%.gz}"
out="$DEST/${TYPE//_/-}.${plain##*.}"         # data/default-cards.jsonl
if [ -f "$out" ] && [ "$FORCE" = 0 ] && [ -n "$(find "$out" -mmin -1440 2>/dev/null)" ]; then
  echo "$out is under 24h old; use --force to re-download"; exit 0
fi

echo "downloading $uri (updated_at $updated)"
curl -fSL -H "User-Agent: $UA" -H "Accept: */*" -o "$out.part" "$uri"
# Gzip by magic bytes, not by name (a server may or may not decompress for us).
if [ "$(head -c2 "$out.part" | od -An -tx1 | tr -d ' \n')" = "1f8b" ]; then
  gunzip -c "$out.part" > "$out.tmp" && mv "$out.tmp" "$out" && rm -f "$out.part"
else
  mv "$out.part" "$out"
fi
head -c 1 "$out" | grep -q '\[' && fmt="JSON array" || fmt="JSON lines"
echo "saved $out ($(du -h "$out" | cut -f1), $fmt, updated_at $updated)"

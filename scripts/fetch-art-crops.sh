#!/usr/bin/env bash
# Download a mixed set of Scryfall art_crop images for `coqc phash-eval` (docs/phash-design.md, P1).
# Usage: scripts/fetch-art-crops.sh OUT_DIR [N=300]
# Picks N art crops: ~1/3 from cards that share very similar art (basic lands, many Lightning Bolt prints),
# the rest random. Polite: descriptive User-Agent, 100 ms between requests, images only (no data is redistributed).
set -euo pipefail
OUT="${1:?usage: fetch-art-crops.sh OUT_DIR [N]}"; N="${2:-300}"
UA="expo-cardboard-compendium-phash-eval/0.0.3 (github.com/ShinyLittleBitsOfPaper/expo-cardboard-compendium)"
mkdir -p "$OUT"; cd "$OUT"
q() { curl -fsS -A "$UA" -H 'Accept: application/json' -G "https://api.scryfall.com/cards/search" --data-urlencode "q=$1" --data-urlencode "unique=art" --data-urlencode "order=$2" | jq -r '.data[]? | select(.image_uris.art_crop) | [.illustration_id, .image_uris.art_crop] | @tsv'; sleep 0.1; }
{
  q '!"Lightning Bolt"' released
  q 't:basic t:land' released
  q '!"Counterspell"' released
  q '!"Llanowar Elves"' released
  q 'r:mythic' random
  q 'r:rare' random
  q 'r:uncommon' random
  q 'r:common' random
  q 'f:modern t:creature' random
  q 't:legendary' random
} | sort -u -k1,1 | shuf | head -n "$N" > list.tsv
i=0
while IFS=$'\t' read -r ill url; do
  i=$((i+1)); curl -fsS -A "$UA" -o "$ill.jpg" "$url" || echo "fail $ill" >&2; sleep 0.1
done < list.tsv
echo "downloaded $(ls *.jpg | wc -l) images to $OUT"

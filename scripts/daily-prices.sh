#!/usr/bin/env bash
# One day of prices: fetch MTGJSON AllPricesToday -> compile against the latest card release -> delta -> verify.
#   scripts/daily-prices.sh [workdir]        (default data/prices; card releases from data/daily/releases)
# Layout: <workdir>/raw/AllPricesToday.json, idmap.coqm, releases/prices-YYYYMMDD.coqr, patches/<old>-to-<new>.coqx
# The uuid map (AllIdentifiers, large) is fetched only when missing, or when more than UNMAPPED_MAX price
# entries are unknown to it and it is older than a day. Env: COQC_USER_AGENT, UNMAPPED_MAX (default 50),
# CARDS (card release to use, default the latest one).
set -euo pipefail
cd "$(dirname "$0")/.."
WORK="${1:-data/prices}"; BIN=./target/release/coqc
UA="${COQC_USER_AGENT:-expo-cardboard-compendium/0.0.2 (set COQC_USER_AGENT to include a contact)}"
[ -x "$BIN" ] || { echo "build first: cargo build --release" >&2; exit 1; }
mkdir -p "$WORK/raw" "$WORK/releases" "$WORK/patches"
CARDS="${CARDS:-$(ls -1 data/daily/releases/cards-*.coqd 2>/dev/null | sort | tail -1)}"
[ -n "$CARDS" ] || { echo "no card release: run scripts/daily.sh first" >&2; exit 1; }

RAW="$WORK/raw/AllPricesToday.json"; IDMAP="$WORK/idmap.coqm"
echo "downloading AllPricesToday"
curl -fSL -H "User-Agent: $UA" -o "$RAW.gz" https://mtgjson.com/api/v5/AllPricesToday.json.gz
gunzip -c "$RAW.gz" > "$RAW" && rm -f "$RAW.gz"
fetch_idmap() {
  echo "downloading AllIdentifiers (large) and building the uuid map"
  curl -fSL -H "User-Agent: $UA" -o "$WORK/raw/AllIdentifiers.json.gz" https://mtgjson.com/api/v5/AllIdentifiers.json.gz
  gunzip -c "$WORK/raw/AllIdentifiers.json.gz" > "$WORK/raw/AllIdentifiers.json"
  "$BIN" idmap "$WORK/raw/AllIdentifiers.json" -o "$IDMAP"
  rm -f "$WORK/raw/AllIdentifiers.json" "$WORK/raw/AllIdentifiers.json.gz"
}
[ -f "$IDMAP" ] || fetch_idmap

STAMP="$(grep -o '"date":"[0-9-]*"' "$RAW" | head -1 | tr -dc 0-9)"
NEW="$WORK/releases/prices-$STAMP.coqr"
if [ -f "$NEW" ]; then echo "$NEW already exists (nothing new since $STAMP)"; exit 0; fi
OUT="$("$BIN" prices "$RAW" --cards "$CARDS" --idmap "$IDMAP" -o "$NEW")"; echo "$OUT"
UNM="$(printf '%s' "$OUT" | sed -n 's/.*unmapped uuids \([0-9]*\).*/\1/p')"
if [ "${UNM:-0}" -gt "${UNMAPPED_MAX:-50}" ] && [ -n "$(find "$IDMAP" -mmin +1440 2>/dev/null)" ]; then
  fetch_idmap; "$BIN" prices "$RAW" --cards "$CARDS" --idmap "$IDMAP" -o "$NEW"
fi
PREV="$(ls -1 "$WORK"/releases/prices-*.coqr | sort | grep -v "$NEW" | tail -1 || true)"
if [ -n "$PREV" ]; then
  PS="$(basename "$PREV" .coqr)"; PS="${PS#prices-}"
  PATCH="$WORK/patches/$PS-to-$STAMP.coqx"
  "$BIN" prices-patch "$PREV" "$NEW" -o "$PATCH"
  TMP="$(mktemp)"; trap 'rm -f "$TMP"' EXIT
  "$BIN" prices-apply "$PREV" "$PATCH" -o "$TMP" | tail -1
  cmp "$TMP" "$NEW" && echo "delta verified"
fi
echo "release: $NEW"

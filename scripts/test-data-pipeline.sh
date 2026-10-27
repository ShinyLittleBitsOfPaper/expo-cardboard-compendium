#!/usr/bin/env bash
# Offline test of the publishing flow (no network, no GitHub): synthetic cards for several "days", the real compile/
# patch/index/manifest code, scripts/data-publish.sh against scripts/mock-gh.sh, then a simulated client that walks
# the manifest's patch chain from day 1 to the latest day and checks every hash. Prices are stand-in files (the real
# price pipeline needs MTGJSON); only their manifest/publish handling is covered here.
#   scripts/test-data-pipeline.sh        (needs target/release/coqc, jq)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="$PWD/target/release/coqc"; [ -x "$BIN" ] || { echo "cargo build --release -p coqc first" >&2; exit 1; }
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
export MOCK_GH_DIR="$T/gh" GH="$PWD/scripts/mock-gh.sh" GITHUB_REPOSITORY=o/r RETAIN_DAYS=30
mkdir -p "$MOCK_GH_DIR"
fail() { echo "FAIL: $*" >&2; exit 1; }
sha() { sha256sum "$1" | cut -d' ' -f1; }

DATES=(2026-09-01 2026-09-02 2026-09-03 2026-10-20)
PREV=""; PPREV=""
for i in 0 1 2 3; do
  d=$((i + 1)); at="${DATES[$i]}"; S="2026090${d}0905"; PS="2026090$d"
  [ "$i" = 3 ] && { S="202610200905"; PS="20261020"; }
  W="$T/w$d"; mkdir -p "$W"
  "$BIN" synth 4000 "$W/raw.jsonl" --day "$d" >/dev/null
  if [ -n "$PREV" ]; then "$BIN" compile "$W/raw.jsonl" -o "$W/cards-$S.coqd" --prev "$PREV" --source-date "${S:0:8}" >/dev/null
  else "$BIN" compile "$W/raw.jsonl" -o "$W/cards-$S.coqd" --source-date "${S:0:8}" >/dev/null; fi
  "$BIN" verify "$W/cards-$S.coqd" "$W/raw.jsonl" >/dev/null
  "$BIN" index-build "$W/cards-$S.coqd" -o "$W/cards-$S.coqs" >/dev/null
  "$BIN" zst "$W/cards-$S.coqs" -o "$W/cards-$S.coqs.zst" --level 3 >/dev/null
  printf 'COQR\001\000\001\000day%s' "$d" > "$W/prices-$PS.coqr"
  ARGS=()
  if [ -n "$PREV" ]; then
    P="$(basename "$PREV" .coqd)"; P="${P#cards-}"
    "$BIN" patch "$PREV" "$W/cards-$S.coqd" -o "$W/cards-$P-to-$S.coqp" >/dev/null
    "$BIN" index-diff "${PREV%.coqd}.coqs" "$W/cards-$S.coqs" -o "$W/cards-$P-to-$S.coqsd" >/dev/null
    printf 'COQXpatch%s' "$d" > "$W/prices-$PPREV-to-$PS.coqx"
    ARGS+=(--cards-patch "$W/cards-$P-to-$S.coqp" --index-delta "$W/cards-$P-to-$S.coqsd" --prices-patch "$W/prices-$PPREV-to-$PS.coqx")
  fi
  [ -f "$T/gh/data-latest/manifest.json" ] && ARGS+=(--prev "$T/gh/data-latest/manifest.json")
  RUNTAG="data-${at//-/}2200"
  "$BIN" manifest --cards "$W/cards-$S.coqd" --index "$W/cards-$S.coqs.zst" --prices "$W/prices-$PS.coqr" --repo o/r \
    --run "$RUNTAG" --at "${at}T22:00:00Z" --keep-since "$(date -u -d "$at - 30 days" +%Y-%m-%dT%H:%M:%SZ)" "${ARGS[@]}" \
    -o "$W/manifest.json" | tee "$W/assets.txt" >/dev/null
  # publish: dry run must not touch anything, the real one must
  PUBLISH=false scripts/data-publish.sh "$W" >/dev/null
  [ "$d" = 1 ] && [ -d "$T/gh/data-latest" ] && fail "dry run published"
  MOCK_GH_NOW="${at}T22:00:00Z" PUBLISH_NOW="${at}T22:00:00Z" scripts/data-publish.sh "$W" >/dev/null
  PREV="$W/cards-$S.coqd"; PPREV="$PS"
  # data-latest holds exactly the current full files + the manifest
  got="$(ls "$T/gh/data-latest" | sort | tr '\n' ' ')"
  want="$(echo "cards-$S.coqd cards-$S.coqs.zst manifest.json prices-$PS.coqr" | tr ' ' '\n' | sort | tr '\n' ' ')"
  [ "$got" = "$want" ] || fail "day $d data-latest: [$got] want [$want]"
  [ "$(sha "$T/gh/data-latest/cards-$S.coqd")" = "$(jq -r .cards.file.sha256 "$T/gh/data-latest/manifest.json")" ] || fail "manifest hash"
  echo "day $d ok ($(jq -c '[.rebase,(.cards.patches|length),(.prices.patches|length)]' "$T/gh/data-latest/manifest.json") rebase,cards chain,prices chain)"
  if [ "$d" = 3 ]; then
    # simulated client: day 1 -> day 3 through the chain, hashes must match the full files
    M="$T/gh/data-latest/manifest.json"; C="$T/client"; mkdir -p "$C"
    url2path() { echo "$MOCK_GH_DIR/$(echo "$1" | sed 's|.*/download/||')"; }
    cp "$T/w1/cards-202609010905.coqd" "$C/cur.coqd"; "$BIN" unzst "$T/w1/cards-202609010905.coqs.zst" -o "$C/cur.coqs" >/dev/null
    for k in $(seq 0 $(( $(jq '.cards.patches|length' "$M") - 1 ))); do
      "$BIN" apply "$C/cur.coqd" "$(url2path "$(jq -r ".cards.patches[$k].patch.url" "$M")")" -o "$C/n.coqd" >/dev/null
      [ "$(sha "$(url2path "$(jq -r ".cards.patches[$k].patch.url" "$M")")")" = "$(jq -r ".cards.patches[$k].patch.sha256" "$M")" ] || fail "patch hash $k"
      "$BIN" index-apply "$C/cur.coqs" "$(url2path "$(jq -r ".cards.patches[$k].index_delta.url" "$M")")" -o "$C/n.coqs" >/dev/null
      mv "$C/n.coqd" "$C/cur.coqd"; mv "$C/n.coqs" "$C/cur.coqs"
    done
    [ "$(sha "$C/cur.coqd")" = "$(jq -r .cards.file.sha256 "$M")" ] || fail "chain end != full cards file"
    [ "$(sha "$C/cur.coqs")" = "$(jq -r .cards.index.raw_sha256 "$M")" ] || fail "chain end != raw index"
    echo "client: day 1 -> day 3 via $(jq '.cards.patches|length' "$M") patches, cards + index hashes match"
  fi
done
# day 4 is 47 days later: old chain links expired, and run releases older than the window were pruned
M="$T/gh/data-latest/manifest.json"
[ "$(jq '.cards.patches|length' "$M")" = 1 ] || fail "expired links not dropped: $(jq -c '[.cards.patches[]|.from]' "$M")"
ls "$T/gh" | grep -q '^data-2026090' && fail "expired run releases not pruned: $(ls "$T/gh")"
ls "$T/gh" | grep -qx data-202610202200 || fail "newest run release missing"
echo "pipeline test passed"

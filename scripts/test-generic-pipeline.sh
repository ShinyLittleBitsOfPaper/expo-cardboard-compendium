#!/usr/bin/env bash
# S5 proof, offline: a NON-MTG dataset (the synthetic books of `coqc demo-books`) through the whole publishing flow with no
# Magic code: schema + JSONL -> `generic-compile` (stable row ids via --prev) -> index -> zstd -> patches and index deltas ->
# price overlay (`generic-overlay`) and its patch -> `coqc manifest` (ids books / books-index / prices) -> data-publish.sh against
# mock-gh -> a simulated client that walks the manifest from day 1 to day 3 (base, index and overlay chains, every hash checked)
# and finally searches the result with an overlay predicate.
#   scripts/test-generic-pipeline.sh        (needs target/release/coqc, jq)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="$PWD/target/release/coqc"; [ -x "$BIN" ] || { echo "cargo build --release -p coqc first" >&2; exit 1; }
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
unset REBASE GH_TOKEN
export MOCK_GH_DIR="$T/gh" GH="$PWD/scripts/mock-gh.sh" GITHUB_REPOSITORY=o/r RETAIN_DAYS=30
mkdir -p "$MOCK_GH_DIR"
fail() { echo "FAIL: $*" >&2; exit 1; }
sha() { sha256sum "$1" | cut -d' ' -f1; }
url2path() { echo "$MOCK_GH_DIR/$(echo "$1" | sed 's|.*/download/||')"; }

PREV=""; PPREV=""
for d in 1 2 3; do
  S="2026110${d}"; at="2026-11-0${d}"
  W="$T/w$d"; mkdir -p "$W"
  "$BIN" demo-books "$W/in" --works 500 --day "$d" >/dev/null
  PA=(); [ -n "$PREV" ] && PA=(--prev "$PREV")
  "$BIN" generic-compile "$W/in/schema.json" --table work="$W/in/works.jsonl" --table edition="$W/in/editions.jsonl" \
    "${PA[@]}" --source-date "$S" -o "$W/books-$S.coqd" >/dev/null
  "$BIN" generic-overlay "$W/in/overlay-schema.json" "$W/books-$S.coqd" "$W/in/prices.jsonl" --source-date "$S" -o "$W/prices-$S.coqd" >/dev/null
  "$BIN" index-build "$W/books-$S.coqd" -o "$W/books-$S.coqi" >/dev/null
  "$BIN" zst "$W/books-$S.coqi" -o "$W/books-$S.coqi.zst" --level 3 >/dev/null
  CP=""; IP=""; PP=""
  if [ -n "$PREV" ]; then
    P="$(basename "$PREV" .coqd)"; P="${P#books-}"
    "$BIN" patch "$PREV" "$W/books-$S.coqd" -o "$W/books-$P-to-$S.coqp" >/dev/null
    "$BIN" index-diff "${PREV%.coqd}.coqi" "$W/books-$S.coqi" -o "$W/books-$P-to-$S.coqid" >/dev/null
    "$BIN" patch "$T/w$((d - 1))/prices-$PPREV.coqd" "$W/prices-$S.coqd" -o "$W/prices-$PPREV-to-$S.coqp" >/dev/null
    CP=",patch=$W/books-$P-to-$S.coqp"; IP=",patch=$W/books-$P-to-$S.coqid"; PP=",patch=$W/prices-$PPREV-to-$S.coqp"
  fi
  MA=(); [ -f "$T/gh/data-latest/manifest.json" ] && MA+=(--prev "$T/gh/data-latest/manifest.json")
  "$BIN" manifest --base "books=$W/books-$S.coqd$CP" --index "books-index=$W/books-$S.coqi.zst,of=books,prefix=books-$IP" \
    --overlay "prices=$W/prices-$S.coqd,of=books$PP" --repo o/r --run "data-${at//-/}2200" --at "${at}T22:00:00Z" \
    --keep-since "$(date -u -d "$at - 30 days" +%Y-%m-%dT%H:%M:%SZ)" "${MA[@]}" -o "$W/manifest.json" | tee "$W/assets.txt" >/dev/null
  PUBLISH=true MOCK_GH_NOW="${at}T22:00:00Z" PUBLISH_NOW="${at}T22:00:00Z" scripts/data-publish.sh "$W" >/dev/null
  PREV="$W/books-$S.coqd"; PPREV="$S"
  got="$(ls "$T/gh/data-latest" | sort | tr '\n' ' ')"
  want="$(printf '%s\n' "books-$S.coqd" "books-$S.coqi.zst" manifest.json "prices-$S.coqd" | sort | tr '\n' ' ')"
  [ "$got" = "$want" ] || fail "day $d data-latest: [$got] want [$want]"
  echo "day $d ok (base chain $(jq '.datasets[0].patches|length' "$T/gh/data-latest/manifest.json"), overlay chain $(jq '.datasets[2].patches|length' "$T/gh/data-latest/manifest.json"))"
done

# simulated client: day 1 -> day 3 through the chains
M="$T/gh/data-latest/manifest.json"; C="$T/client"; mkdir -p "$C"
[ "$(jq -r '[.datasets[]|.id]|join(",")' "$M")" = "books,books-index,prices" ] || fail "dataset ids"
cp "$T/w1/books-20261101.coqd" "$C/cur.coqd"; "$BIN" unzst "$T/w1/books-20261101.coqi.zst" -o "$C/cur.coqi" >/dev/null
cp "$T/w1/prices-20261101.coqd" "$C/prices.coqd"
n=$(jq '.datasets[0].patches|length' "$M"); [ "$n" = 2 ] || fail "expected 2 base links, got $n"
for k in $(seq 0 $((n - 1))); do
  link() { url2path "$(jq -r ".datasets[$1].patches[$k].patch.url" "$M")"; }
  [ "$(sha "$(link 0)")" = "$(jq -r ".datasets[0].patches[$k].patch.sha256" "$M")" ] || fail "base patch hash $k"
  "$BIN" apply "$C/cur.coqd" "$(link 0)" -o "$C/n.coqd" >/dev/null
  "$BIN" index-apply "$C/cur.coqi" "$(link 1)" -o "$C/n.coqi" >/dev/null
  "$BIN" apply "$C/prices.coqd" "$(link 2)" -o "$C/np.coqd" >/dev/null
  mv "$C/n.coqd" "$C/cur.coqd"; mv "$C/n.coqi" "$C/cur.coqi"; mv "$C/np.coqd" "$C/prices.coqd"
done
[ "$(sha "$C/cur.coqd")" = "$(jq -r .datasets[0].file.sha256 "$M")" ] || fail "base chain end != full file"
[ "$(sha "$C/cur.coqi")" = "$(jq -r .datasets[1].raw.sha256 "$M")" ] || fail "index chain end != raw index"
[ "$(sha "$C/prices.coqd")" = "$(jq -r .datasets[2].file.sha256 "$M")" ] || fail "overlay chain end != full file"
OUT="$("$BIN" search "$C/cur.coqd" 'price<1500 format:ebook subject:scifi' --overlay "prices=$C/prices.coqd" --limit 1)"
echo "$OUT" | head -1 | grep -Eq '^[1-9][0-9]* matches' || fail "client search found nothing: $OUT"
echo "client: day 1 -> day 3 via $n patches (base, index, overlay), hashes match; $(echo "$OUT" | head -1)"
echo "generic pipeline test passed"

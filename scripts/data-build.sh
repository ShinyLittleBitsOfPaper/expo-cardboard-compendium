#!/usr/bin/env bash
# Milestone 1, phase A: build one data run into a stage directory (no publishing).
#
#   scripts/data-build.sh [stage_dir]          (default data/stage)
#
# 1. restore the current release from `data-latest` (row stability via --prev, patch base); none => first release
# 2. scripts/daily.sh + scripts/daily-prices.sh (compile, patch, index, price delta; each verifies its own replay)
# 3. `coqc verify` of the new cards file against the raw bulk file (0 mismatches required)
# 4. compress the index (.coqi.zst) and write manifest.json
# Result: <stage>/ holds manifest.json, assets.txt, the full files and this run's patches; or <stage>/NOTHING when
# neither Scryfall nor MTGJSON has new data (idempotent: the workflow then ends cleanly).
# Env: GITHUB_REPOSITORY (OWNER/REPO, required), COQC_USER_AGENT, RETAIN_DAYS (30), REBASE=1 (force a new chain),
#      MANIFEST_URL (override, testing). Needs: cargo-built target/release/coqc, curl, jq, sha256sum.
set -euo pipefail
cd "$(dirname "$0")/.."
STAGE="${1:-data/stage}"; BIN=./target/release/coqc
REPO="${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY=OWNER/REPO}"; RETAIN_DAYS="${RETAIN_DAYS:-30}"
BASE="https://github.com/$REPO/releases/download"
MURL="${MANIFEST_URL:-$BASE/data-latest/manifest.json}"
[ -x "$BIN" ] || { echo "build first: cargo build --release -p coqc" >&2; exit 1; }
rm -rf "$STAGE"; mkdir -p "$STAGE" data/daily/releases data/prices/releases
PREVM="$STAGE/prev-manifest.json"

fetch() {  # url dest sha256
  curl -fSL --retry 3 -o "$2" "$1"
  [ "$(sha256sum "$2" | cut -d' ' -f1)" = "$3" ] || { echo "hash mismatch for $1" >&2; exit 1; }
}

echo "== restoring the current release"
if curl -fsSL --retry 3 -o "$PREVM" "$MURL"; then
  PSTAMP="$(jq -r .cards.stamp "$PREVM")"; PPRICE="$(jq -r .prices.stamp "$PREVM")"
  echo "current: cards $PSTAMP prices $PPRICE (epoch $(jq -r .epoch "$PREVM"))"
  CF="data/daily/releases/$(jq -r .cards.file.name "$PREVM")"
  [ -f "$CF" ] || fetch "$(jq -r .cards.file.url "$PREVM")" "$CF" "$(jq -r .cards.file.sha256 "$PREVM")"
  PRF="data/prices/releases/$(jq -r .prices.file.name "$PREVM")"
  [ -f "$PRF" ] || fetch "$(jq -r .prices.file.url "$PREVM")" "$PRF" "$(jq -r .prices.file.sha256 "$PREVM")"
  IDX="${CF%.coqd}.coqi"
  if [ ! -f "$IDX" ]; then
    fetch "$(jq -r .cards.index.url "$PREVM")" "$IDX.zst" "$(jq -r .cards.index.sha256 "$PREVM")"
    "$BIN" unzst "$IDX.zst" -o "$IDX" >/dev/null; rm -f "$IDX.zst"
    [ "$(sha256sum "$IDX" | cut -d' ' -f1)" = "$(jq -r .cards.index.raw_sha256 "$PREVM")" ] || { echo "index hash mismatch" >&2; exit 1; }
  fi
else
  rm -f "$PREVM"; PSTAMP=""; PPRICE=""; echo "no published release yet: first release"
fi

scripts/daily.sh
scripts/daily-prices.sh

CARDS="$(ls -1 data/daily/releases/cards-*.coqd | sort | tail -1)"; PRICES="$(ls -1 data/prices/releases/prices-*.coqr | sort | tail -1)"
CS="$(basename "$CARDS" .coqd)"; CS="${CS#cards-}"; PS="$(basename "$PRICES" .coqr)"; PS="${PS#prices-}"
if [ "$CS" = "$PSTAMP" ] && [ "$PS" = "$PPRICE" ]; then
  echo "nothing new (cards $CS, prices $PS)"; touch "$STAGE/NOTHING"; exit 0
fi

echo "== verifying $CARDS against the raw bulk file"
"$BIN" verify "$CARDS" data/daily/raw/default-cards.jsonl | tail -3

echo "== staging"
cp "$CARDS" "$STAGE/"; cp "$PRICES" "$STAGE/"
"$BIN" zst "${CARDS%.coqd}.coqi" -o "$STAGE/cards-$CS.coqi.zst" --level 19
ARGS=()
if [ -n "$PSTAMP" ] && [ "$CS" != "$PSTAMP" ]; then
  cp "data/daily/patches/$PSTAMP-to-$CS.coqp" "data/daily/patches/$PSTAMP-to-$CS.coqid" "$STAGE/"
  ARGS+=(--cards-patch "$STAGE/cards-$PSTAMP-to-$CS.coqp" --index-delta "$STAGE/cards-$PSTAMP-to-$CS.coqid")
fi
if [ -n "$PPRICE" ] && [ "$PS" != "$PPRICE" ]; then
  cp "data/prices/patches/$PPRICE-to-$PS.coqx" "$STAGE/"
  ARGS+=(--prices-patch "$STAGE/prices-$PPRICE-to-$PS.coqx")
fi
[ -f "$PREVM" ] && ARGS+=(--prev "$PREVM")
[ "${REBASE:-0}" = 1 ] && ARGS+=(--rebase)
"$BIN" manifest --cards "$STAGE/cards-$CS.coqd" --index "$STAGE/cards-$CS.coqi.zst" --prices "$STAGE/prices-$PS.coqr" \
  --repo "$REPO" --run "data-$(date -u +%Y%m%d%H%M)" --at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --keep-since "$(date -u -d "$RETAIN_DAYS days ago" +%Y-%m-%dT%H:%M:%SZ)" "${ARGS[@]}" -o "$STAGE/manifest.json" | tee "$STAGE/assets.txt"

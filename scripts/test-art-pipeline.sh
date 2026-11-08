#!/usr/bin/env bash
# Offline test of the art-hash overlay step (scripts/data-art.sh): synthetic cards, a fake hash cache, no network, no images.
# Checks: overlay built from the cache, spec written, day-2 patch verified against the previous overlay, a missing cache is
# tolerated before the first publish and refused afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=./target/release/coqc; [ -x "$BIN" ] || { echo "build first: cargo build --release -p coqc" >&2; exit 1; }
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
export BIN ART_WORK="$T/work" GITHUB_REPOSITORY=test/test
ok() { echo "ok: $*"; }
"$BIN" synth 400 "$T/d1.jsonl" --day 1 >/dev/null
"$BIN" compile "$T/d1.jsonl" -o "$T/cards-20261101.coqd" >/dev/null
# fake cache for ~90% of the illustrations
"$BIN" art-hash "$T/d1.jsonl" --cards "$T/cards-20261101.coqd" --cache "$T/empty.tsv" -o "$T/x.coqd" --todo "$T/todo.tsv" >/dev/null
awk -F'\t' 'NR % 10 != 0 {printf "%s\t%016x\n", $1, (NR * 2654435761) % 18446744073709551615}' "$T/todo.tsv" > "$T/cache.tsv"
"$BIN" zst "$T/cache.tsv" -o "$T/art-hashes.tsv.zst" >/dev/null
export ART_CACHE_URL="file://$T/art-hashes.tsv.zst"

# day 1: no previous manifest
mkdir -p "$T/s1"; BULK="$T/d1.jsonl" scripts/data-art.sh "$T/s1" "$T/cards-20261101.coqd" 20261101 >/dev/null
SPEC="$(cat "$T/s1/art.spec")"; [[ "$SPEC" == arthash=*arthash-20261101.coqd,of=cards ]] || { echo "bad spec: $SPEC" >&2; exit 1; }
[ -s "$T/s1/arthash-20261101.coqd" ] || { echo "no overlay" >&2; exit 1; }
ok "day 1 overlay + spec ($(stat -c %s "$T/s1/arthash-20261101.coqd") bytes)"

# day 2: new cards file, previous manifest lists the published overlay
"$BIN" synth 400 "$T/d2.jsonl" --day 2 >/dev/null
"$BIN" compile "$T/d2.jsonl" -o "$T/cards-20261102.coqd" --prev "$T/cards-20261101.coqd" >/dev/null
cat > "$T/prev.json" <<JSON
{"manifest_version":2,"datasets":[{"id":"arthash","stamp":"20261101","file":{"name":"arthash-20261101.coqd","url":"file://$T/s1/arthash-20261101.coqd","sha256":"$(sha256sum "$T/s1/arthash-20261101.coqd" | cut -d' ' -f1)"}}]}
JSON
mkdir -p "$T/s2"; BULK="$T/d2.jsonl" scripts/data-art.sh "$T/s2" "$T/cards-20261102.coqd" 20261102 "$T/prev.json" | tee "$T/s2.log" >/dev/null
SPEC="$(cat "$T/s2/art.spec")"
[[ "$SPEC" == *",patch=$T/s2/arthash-20261101-to-20261102.coqp" ]] || { echo "no patch in spec: $SPEC" >&2; cat "$T/s2.log" >&2; exit 1; }
ok "day 2 patch ($(stat -c %s "$T/s2/arthash-20261101-to-20261102.coqp") bytes), replay verified"

# the manifest accepts it as a second overlay next to prices
"$BIN" index-build "$T/cards-20261102.coqd" -o "$T/cards-20261102.coqi" >/dev/null
"$BIN" zst "$T/cards-20261102.coqi" -o "$T/cards-20261102.coqi.zst" >/dev/null
cp "$T/s2/arthash-20261102.coqd" "$T/prices-20261102.coqd"   # stand-in prices overlay (only the manifest handling matters here)
"$BIN" manifest --repo test/test --run data-test --at 2026-11-02T22:00:00Z --keep-since 2026-10-01T00:00:00Z \
  --base "cards=$T/cards-20261102.coqd" --index "cards-index=$T/cards-20261102.coqi.zst,of=cards,prefix=cards-" \
  --overlay "prices=$T/prices-20261102.coqd,of=cards" --overlay "$SPEC" -o "$T/manifest.json" >/dev/null
[ "$(jq -r '[.datasets[] | select(.id == "arthash") | .role + ":" + .of] | .[0]' "$T/manifest.json")" = "overlay:cards" ] || { echo "arthash not an overlay in the manifest" >&2; exit 1; }
ok "manifest lists arthash as a second overlay: $(jq -r '[.datasets[].id] | join(",")' "$T/manifest.json")"

# missing cache: fine before the first publish, refused after it
export ART_CACHE_URL="file://$T/missing.zst"
mkdir -p "$T/s3"; BULK="$T/d2.jsonl" scripts/data-art.sh "$T/s3" "$T/cards-20261102.coqd" 20261102 >/dev/null 2>&1
[ ! -s "$T/s3/art.spec" ] || { echo "spec without a cache" >&2; exit 1; }
ok "no cache, nothing published yet: no overlay, no failure"
mkdir -p "$T/s4"
if BULK="$T/d2.jsonl" scripts/data-art.sh "$T/s4" "$T/cards-20261102.coqd" 20261102 "$T/prev.json" >/dev/null 2>&1; then echo "dropped a published overlay" >&2; exit 1; fi
ok "no cache but arthash already published: refused"
echo "art pipeline test passed"

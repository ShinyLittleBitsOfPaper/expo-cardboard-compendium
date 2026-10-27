#!/usr/bin/env bash
# Minimal stand-in for `gh release ...` for scripts/test-data-pipeline.sh. State lives in $MOCK_GH_DIR/<tag>/ (assets as
# files, .meta = "draft createdAt"). Only the calls scripts/data-publish.sh makes are implemented.
set -euo pipefail
D="${MOCK_GH_DIR:?}"; NOW="${MOCK_GH_NOW:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
[ "$1" = release ] || { echo "mock: unsupported $*" >&2; exit 2; }
sub="$2"; shift 2
tag=""; files=(); draft=false; clob=0
while [ $# -gt 0 ]; do
  case "$1" in
    --repo|--title|--notes|--limit|--json) shift ;;
    --draft) draft=true ;; --draft=false) draft=false ;;
    --prerelease|--latest=false|--clobber|--yes|--cleanup-tag) ;;
    -*) echo "mock: unsupported flag $1" >&2; exit 2 ;;
    *) if [ -z "$tag" ] && [ "$sub" != list ]; then tag="$1"; else files+=("$1"); fi ;;
  esac; shift
done
case "$sub" in
  view)   [ -d "$D/$tag" ] || exit 1
          # only `--json assets` is used
          (cd "$D/$tag" && for f in *; do [ -e "$f" ] && printf '{"name":"%s","size":%s}\n' "$f" "$(stat -c %s "$f")"; done) | jq -s '{assets:.}' ;;
  create) [ ! -d "$D/$tag" ] || { echo "mock: $tag exists" >&2; exit 1; }
          mkdir -p "$D/$tag"; echo "$draft $NOW" > "$D/$tag/.meta"; for f in "${files[@]}"; do cp "$f" "$D/$tag/"; done ;;
  upload) [ -d "$D/$tag" ] || exit 1; for f in "${files[@]}"; do cp "$f" "$D/$tag/"; done ;;
  edit)   [ -d "$D/$tag" ] || exit 1; sed -i "s/^[a-z]*/$draft/" "$D/$tag/.meta" ;;
  delete-asset) rm -f "$D/$tag/${files[0]}" ;;
  delete) rm -rf "$D/$tag" ;;
  list)   for t in "$D"/*/; do [ -d "$t" ] || continue; n="$(basename "$t")"; printf '{"tagName":"%s","createdAt":"%s"}\n' "$n" "$(cut -d' ' -f2 "$t/.meta")"; done | jq -s . ;;
  *) echo "mock: unsupported release $sub" >&2; exit 2 ;;
esac

#!/usr/bin/env bash
# Package the working tree as a bundle to hand to Claude:
#   bundles/expo-scryfall-engine-<version>-<N>.tar.gz
#
#   scripts/package-bundle.sh [--no-bump] [--out DIR]
#
# N is the package number from .bundle-seq, bumped on every run (--no-bump to repackage
# the same number). Contents follow git: tracked files plus untracked files that are not
# ignored, so target/, data/, bundles/ and *.coqd never go in. Needs GNU tar.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="bundles"; BUMP=1
while [ $# -gt 0 ]; do
  case "$1" in
    --no-bump) BUMP=0 ;;
    --out) OUT="$2"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

cargo_version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
seq="$(cat .bundle-seq 2>/dev/null || echo "$cargo_version-0")"
version="${seq%-*}"; n="${seq##*-}"
if [ "$version" != "$cargo_version" ]; then version="$cargo_version"; n=0; fi   # new release restarts at -1
if [ "$BUMP" = 1 ]; then n=$((n + 1)); fi
echo "$version-$n" > .bundle-seq
name="expo-scryfall-engine-$version-$n"

mkdir -p "$OUT"
list="$(mktemp)"; trap 'rm -f "$list"' EXIT
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git ls-files -co --exclude-standard -z | while IFS= read -r -d '' f; do
    [ -e "$f" ] && printf '%s\0' "$f"
  done > "$list"
else
  echo "not a git repo: packaging everything except target/ data/ bundles/ .git/" >&2
  find . -type f -not -path './target/*' -not -path './data/*' -not -path './bundles/*' -not -path './.git/*' -print0 \
    | sed -z 's,^\./,,' > "$list"
fi

tar --transform 's,^,expo-scryfall-engine/,' --null -T "$list" -czf "$OUT/$name.tar.gz"
echo "created $OUT/$name.tar.gz ($(du -h "$OUT/$name.tar.gz" | cut -f1), $(tr -cd '\0' < "$list" | wc -c) files)"
echo "attach it to your next message and tell me the name: $name"

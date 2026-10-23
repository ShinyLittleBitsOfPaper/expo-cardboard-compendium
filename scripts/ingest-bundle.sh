#!/usr/bin/env bash
# Apply a bundle from Claude to this working tree.
#
#   scripts/ingest-bundle.sh <bundle.tar.gz> [--prune] [--force]
#
# Copies new and changed files over the repo. Never touches .git/, target/, data/ or bundles/,
# and never deletes your untracked files. Files that were tracked here but are missing from the
# bundle are only listed unless you pass --prune (then they are deleted).
# Refuses to run with uncommitted changes (so the bundle shows up as a clean `git diff`)
# unless you pass --force.
set -euo pipefail
cd "$(dirname "$0")/.."

bundle=""; PRUNE=0; FORCE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --prune) PRUNE=1 ;;
    --force) FORCE=1 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) bundle="$1" ;;
  esac
  shift
done
[ -n "$bundle" ] && [ -f "$bundle" ] || { echo "usage: $0 <bundle.tar.gz> [--prune] [--force]" >&2; exit 2; }
[ -f Cargo.toml ] || { echo "run from the expo-scryfall-engine repo" >&2; exit 2; }

in_git=0; git rev-parse --is-inside-work-tree >/dev/null 2>&1 && in_git=1
if [ "$in_git" = 1 ] && [ "$FORCE" = 0 ] && [ -n "$(git status --porcelain)" ]; then
  echo "uncommitted changes present. Commit or stash them first, or use --force." >&2
  git status --short >&2
  exit 1
fi

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
tar -xzf "$bundle" -C "$tmp"
top="$(ls -A "$tmp")"
[ "$(printf '%s\n' "$top" | wc -l)" = 1 ] && [ -d "$tmp/$top" ] || { echo "unexpected bundle layout (want one top-level folder)" >&2; exit 1; }
src="$tmp/$top"
rm -rf "$src/.git" "$src/target" "$src/data" "$src/bundles"

cp -a "$src/." ./
echo "applied $(basename "$bundle") (bundle id: $(cat .bundle-seq 2>/dev/null || echo unknown))"

if [ "$in_git" = 1 ]; then
  gone=()
  while IFS= read -r -d '' f; do
    [ -e "$src/$f" ] || gone+=("$f")
  done < <(git ls-files -z)
  if [ "${#gone[@]}" -gt 0 ]; then
    if [ "$PRUNE" = 1 ]; then
      printf 'deleting (not in bundle): %s\n' "${gone[@]}"
      rm -f -- "${gone[@]}"
    else
      printf 'tracked here but not in the bundle (use --prune to delete): %s\n' "${gone[@]}"
    fi
  fi
  echo "--- git status ---"; git status --short
fi
echo "next: cargo test && cargo build --release"

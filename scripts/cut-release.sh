#!/usr/bin/env bash
# Cut a code release: bump the version, commit, tag vX.Y.Z and push the commit and the tag to GitHub.
#   scripts/cut-release.sh major|minor|patch [--dry-run]
# The tag push is what starts the release build (.github/workflows/native.yml). The data pipeline is separate (docs/release.md).
# Steps: check preconditions -> scripts/set-version.sh (every version field + Cargo.lock) -> commit "Release vX.Y.Z" -> annotated tag
# -> `git push --atomic <remote> <branch> vX.Y.Z`. Nothing is pushed if any earlier step fails; --dry-run only prints the plan.
# Overrides: RELEASE_BRANCH (default trunk), RELEASE_REMOTE (default origin).
set -euo pipefail
cd "$(dirname "$0")/.."

BUMP="${1:-}"
DRY=0
[ "${2:-}" = "--dry-run" ] && DRY=1
case "$BUMP" in major|minor|patch) ;; *) echo "usage: scripts/cut-release.sh major|minor|patch [--dry-run]" >&2; exit 2 ;; esac
[ $# -le 2 ] || { echo "usage: scripts/cut-release.sh major|minor|patch [--dry-run]" >&2; exit 2; }
[ $# -lt 2 ] || [ "$2" = "--dry-run" ] || { echo "unknown option: $2" >&2; exit 2; }

BRANCH="${RELEASE_BRANCH:-trunk}"
REMOTE="${RELEASE_REMOTE:-origin}"

CUR="$(sed -nE '0,/^version = "([0-9]+\.[0-9]+\.[0-9]+)"/s//\1/p' Cargo.toml | head -1)"
[[ "$CUR" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || { echo "cannot read the current X.Y.Z version from Cargo.toml (got '$CUR')" >&2; exit 1; }
MA="${BASH_REMATCH[1]}"; MI="${BASH_REMATCH[2]}"; PA="${BASH_REMATCH[3]}"
case "$BUMP" in
  major) MA=$((MA + 1)); MI=0; PA=0 ;;
  minor) MI=$((MI + 1)); PA=0 ;;
  patch) PA=$((PA + 1)) ;;
esac
NEXT="$MA.$MI.$PA"
TAG="v$NEXT"
echo "release: $CUR -> $NEXT ($BUMP), tag $TAG, branch $BRANCH, remote $REMOTE"

# ---- preconditions -------------------------------------------------------------------------------------------------
[ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] || { echo "not on '$BRANCH' (set RELEASE_BRANCH to release from another branch)" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "working tree is not clean; commit or stash first" >&2; git status --short >&2; exit 1; }
git remote get-url "$REMOTE" >/dev/null 2>&1 || { echo "no git remote '$REMOTE'" >&2; exit 1; }
git fetch --quiet "$REMOTE" "$BRANCH" --tags
if git rev-parse --quiet --verify "refs/tags/$TAG" >/dev/null; then echo "tag $TAG already exists locally" >&2; exit 1; fi
if [ -n "$(git ls-remote --tags "$REMOTE" "refs/tags/$TAG")" ]; then echo "tag $TAG already exists on $REMOTE" >&2; exit 1; fi
if git rev-parse --quiet --verify "refs/remotes/$REMOTE/$BRANCH" >/dev/null; then
  read -r BEHIND AHEAD < <(git rev-list --left-right --count "$REMOTE/$BRANCH...HEAD")
  [ "$BEHIND" = 0 ] || { echo "$BRANCH is $BEHIND commit(s) behind $REMOTE/$BRANCH; pull first" >&2; exit 1; }
  echo "$BRANCH is up to date with $REMOTE ($AHEAD local commit(s) will be pushed)"
fi

if [ "$DRY" = 1 ]; then
  echo "dry run: would run scripts/set-version.sh $NEXT, commit 'Release $TAG', tag $TAG and push $BRANCH + $TAG to $REMOTE"
  exit 0
fi

# ---- bump, commit, tag, push ---------------------------------------------------------------------------------------
scripts/set-version.sh "$NEXT"
scripts/check-version.sh "$TAG"
git add -u
git commit --quiet -m "Release $TAG"
git tag -a "$TAG" -m "Release $TAG"
git push --atomic "$REMOTE" "$BRANCH" "$TAG"
echo "pushed $TAG; the release build runs in GitHub Actions (native.yml)"

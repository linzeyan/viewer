#!/usr/bin/env bash
# make release VERSION=x.y.z: sets Cargo.toml's version, runs the checks, commits, tags and
# pushes. Pushing the tag starts .github/workflows/release.yml, which builds and publishes.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${1:?usage: scripts/release.sh x.y.z}
TAG="v$VERSION"
fail() { echo "release: $*" >&2; exit 1; }

[[ $VERSION =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "$VERSION is not x.y.z"
[[ $(git branch --show-current) == main ]] || fail "not on main"
[[ -z $(git status --porcelain) ]] || fail "the working tree has changes"
git fetch --quiet origin
# origin/main doesn't exist before the first push.
if git rev-parse --verify --quiet origin/main >/dev/null; then
  git merge-base --is-ancestor origin/main HEAD || fail "main is behind origin/main"
fi
if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null || [[ -n $(git ls-remote --tags origin "refs/tags/$TAG") ]]; then
  fail "$TAG already exists"
fi

current=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
if [[ $current != "$VERSION" ]]; then
  # The package's version is the only top-level `version = ` line; dependencies are inline.
  sed -i '' "1,/^version = /s/^version = \".*\"$/version = \"$VERSION\"/" Cargo.toml
  cargo update --workspace --offline --quiet
fi

make check

if [[ $current != "$VERSION" ]]; then
  git add Cargo.toml Cargo.lock
  git commit --quiet -m "Release $TAG"
fi
# A tag pushed without its version change would publish the wrong version.
[[ -z $(git status --porcelain) ]] || fail "uncommitted changes after the release commit"
git show HEAD:Cargo.toml | grep -qx "version = \"$VERSION\"" || fail "HEAD's Cargo.toml is not $VERSION"
git show HEAD:Cargo.lock | grep -A1 -x 'name = "viewer"' | grep -qx "version = \"$VERSION\"" ||
  fail "HEAD's Cargo.lock is not $VERSION"

git tag -a "$TAG" -m "$TAG"
echo
git log --oneline -1
read -r -p "Push main and $TAG to origin? [y/N] " answer
[[ $answer == [yY] ]] || { echo "Not pushed. Undo with: git tag -d $TAG"; exit 1; }
git push origin main
# Alone: GitHub sends no tag event when more than three tags arrive in one push.
git push origin "refs/tags/$TAG"
echo "Building: https://github.com/linzeyan/viewer/actions"

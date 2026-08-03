#!/bin/bash
set -euo pipefail

VERSION_TYPE="${1:-patch}"

# Bump workspace version
cargo install cargo-workspaces --locked 2>/dev/null || true
cargo ws version "$VERSION_TYPE" --no-git-commit || {
  # Fallback: manual version bump
  CURRENT=$(grep '^version' oxicode-cli/Cargo.toml | head -1 | cut -d'"' -f2)
  IFS='.' read -r major minor patch <<< "$CURRENT"
  case "$VERSION_TYPE" in
    major) NEW_VERSION="$((major+1)).0.0" ;;
    minor) NEW_VERSION="${major}.$((minor+1)).0" ;;
    *) NEW_VERSION="${major}.${minor}.$((patch+1))" ;;
  esac
  echo "Would bump version: $CURRENT -> $NEW_VERSION"
  echo "Run manually: sed -i 's/version = \"$CURRENT\"/version = \"$NEW_VERSION\"/' Cargo.toml oxicode-*/Cargo.toml"
  exit 1
}

NEW_VERSION=$(grep '^version' oxicode-cli/Cargo.toml | head -1 | cut -d'"' -f2)
echo "New version: $NEW_VERSION"

cargo generate-lockfile

git add -A
git commit -m "chore: release v$NEW_VERSION"
git tag "v$NEW_VERSION"
echo "Ready to push: git push origin main --tags"

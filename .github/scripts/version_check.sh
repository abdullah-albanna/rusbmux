#!/usr/bin/env bash
#
# Reads the package version from Cargo.toml and compares it against the latest
# GitHub release tag to decide whether a release should be built.
#
# Writes to $GITHUB_OUTPUT:
#   version       - the version from Cargo.toml
#   should_release - true/false
#
# Env:
#   FORCE_RELEASE   set to "true" to always release (used by workflow_dispatch)
#   GITHUB_REPOSITORY owner/repo (only the releases/latest lookup)
set -euo pipefail

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ -z "$VERSION" ]; then
  echo "Could not determine version from Cargo.toml"
  exit 1
fi

echo "Current version: $VERSION"
echo "version=$VERSION" >>"$GITHUB_OUTPUT"

if [ "${FORCE_RELEASE:-false}" = "true" ]; then
  echo "Manual workflow run; forcing release"
  echo "should_release=true" >>"$GITHUB_OUTPUT"
  exit 0
fi

if ! LATEST_TAG=$(gh api "repos/${GITHUB_REPOSITORY}/releases/latest" \
  --jq '.tag_name' 2>/tmp/gh-err); then
  if grep -qE 'HTTP 404' /tmp/gh-err; then
    LATEST_TAG=""
  else
    echo "GitHub API failed:"
    cat /tmp/gh-err
    exit 1
  fi
fi

PUBLISHED=${LATEST_TAG#v}

if [ -z "$PUBLISHED" ]; then
  echo "No releases yet; this will be the first release"
  echo "should_release=true" >>"$GITHUB_OUTPUT"
  exit 0
fi

echo "Latest release version: $PUBLISHED"

NEWEST=$(printf '%s\n%s\n' "$PUBLISHED" "$VERSION" | sort -V | tail -n 1)

if [ "$VERSION" = "$NEWEST" ] && [ "$VERSION" != "$PUBLISHED" ]; then
  echo "Version newer than latest release: $PUBLISHED -> $VERSION"
  echo "should_release=true" >>"$GITHUB_OUTPUT"
else
  echo "Version is not newer than the latest release ($PUBLISHED)"
  echo "should_release=false" >>"$GITHUB_OUTPUT"
fi

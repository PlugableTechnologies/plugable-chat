#!/usr/bin/env bash
# Publish the CPU embedding model files as assets of a dedicated prerelease, so installed apps
# can fetch them from GitHub instead of huggingface.co (which some networks reset).
#
# Source of truth: src-tauri/src/actors/foundry/embedding_model_manifest.json (pinned upstream
# revision + SHA256 of every file + the release tag). The script downloads each file from the
# pinned revision, refuses to continue on any hash mismatch, then uploads the files plus
# SHA256SUMS and the upstream model card (MIT licence notice) to the release.
#
# The release is created with --prerelease and --latest=false so it never becomes the project's
# "Latest" release. Re-running is safe: existing assets are replaced (--clobber).
#
# Usage: scripts/ci/publish-embedding-model.sh [--dry-run] [--repo OWNER/REPO]
set -euo pipefail

repo="PlugableTechnologies/plugable-chat"
dry_run=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --repo) repo="$2"; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

root="$(cd "$(dirname "$0")/../.." && pwd)"
manifest="$root/src-tauri/src/actors/foundry/embedding_model_manifest.json"
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
command -v gh >/dev/null || { echo "gh is required" >&2; exit 1; }

upstream="$(jq -r .upstream_repo "$manifest")"
revision="$(jq -r .revision "$manifest")"
tag="$(jq -r .release_tag "$manifest")"
sha256_of() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

: > "$work/SHA256SUMS"
count="$(jq '.files | length' "$manifest")"
for i in $(seq 0 $((count - 1))); do
  path="$(jq -r ".files[$i].path" "$manifest")"
  asset="$(jq -r ".files[$i].asset" "$manifest")"
  want="$(jq -r ".files[$i].sha256" "$manifest")"
  echo "downloading $path at $revision"
  curl -fL --retry 5 --retry-delay 3 -C - -o "$work/$asset" "https://huggingface.co/$upstream/resolve/$revision/$path"
  got="$(sha256_of "$work/$asset")"
  if [ "$got" != "$want" ]; then
    echo "SHA256 mismatch for $path: expected $want, got $got" >&2
    exit 1
  fi
  echo "$got  $asset" >> "$work/SHA256SUMS"
done
curl -fL --retry 5 -o "$work/MODEL-CARD.md" "https://huggingface.co/$upstream/resolve/$revision/README.md"
grep -q '^license: mit' "$work/MODEL-CARD.md" || { echo "model card no longer says license: mit" >&2; exit 1; }

if [ "$dry_run" = 1 ]; then
  echo "dry run: verified $count files, would publish to $repo tag $tag"
  exit 0
fi

notes="CPU embedding model for Plugable Chat search: $upstream at the pinned revision $revision (MIT licence, see MODEL-CARD.md). Files are byte-identical to the upstream ones; SHA256SUMS lists their hashes and the app verifies them before use. Not an application release."
if gh release view "$tag" --repo "$repo" >/dev/null 2>&1; then
  echo "release $tag exists; uploading assets"
else
  gh release create "$tag" --repo "$repo" --prerelease --latest=false \
    --title "Embedding model bge-base-en-v1.5 (r1)" --notes "$notes" --target main
fi
gh release upload "$tag" --repo "$repo" --clobber "$work"/*
gh release view "$tag" --repo "$repo" --json url,isPrerelease,assets --jq '{url, isPrerelease, assets: [.assets[] | {name, size}]}'

#!/bin/bash
# Publish a runtime bundle as a GitHub release, so that any machine can install it
# with one command and no image, no checkout and no build tools.
#
#   tools/publish-runtime.sh [--dry-run] [--draft] [--repo owner/name] [--tag TAG]
#                            [--version V] [bundle-directory]
#
# --draft uploads the assets without making the release visible: nothing is listed,
# no tag is created until it is published, and removing it is one command. It is how
# the upload path can be tested without publishing Android's binaries under your own
# name, and how a release can be prepared before it goes out.
#
# What it does:
#
#   1. packs the bundle (tools/bundle/bundle.sh pack)
#   2. creates the release and uploads the archive and its sha256
#   3. prints the two lines a fresh machine needs
#
# After that, another machine gets a working runtime with:
#
#   mosaic runtime install https://github.com/<repo>/releases/download/<tag>/runtime-<v>-<arch>.tar.xz
#
# or, for every machine of that architecture at once, by pointing the channel at the
# release in <work>/mosaic.cfg:
#
#   bundle_channel = https://github.com/<repo>/releases/latest/download
#   bundle_version = <v>
#
# Read this before publishing, because it matters more than the mechanics: a bundle
# contains the system image's ART, Bionic and framework jars -- Android's own
# binaries, Apache-2.0, unmodified, plus whatever the image's distribution added.
# Publishing them under your name is a distribution decision, and the reason this is
# a script you run rather than something the build does on its own. Waydroid
# publishes comparable images publicly, so there is precedent; the choice is yours.

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)

repo=""
tag=""
version="local"
dry_run=0
draft=0
bundle=""

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry_run=1 ;;
    --draft) draft=1 ;;
    --repo) repo="${2:?--repo needs owner/name}" && shift ;;
    --tag) tag="${2:?--tag needs a value}" && shift ;;
    --version) version="${2:?--version needs a value}" && shift ;;
    -h|--help) usage ;;
    -*) echo "unknown option: $1" >&2 && usage ;;
    *) bundle="$1" ;;
  esac
  shift
done

# Where the bundle is, if it was not given: this user's, then the machine's.
if [ -z "$bundle" ]; then
  for candidate in "${XDG_DATA_HOME:-$HOME/.local/share}/mosaic/runtime" \
                   /var/lib/mosaic/runtime; do
    if [ -f "$candidate/version" ]; then
      v=$(tr -d '\n' < "$candidate/version")
      arch=$(uname -m); case "$arch" in aarch64|arm64) arch=aarch64 ;; *) arch=x86_64 ;; esac
      if [ -d "$candidate/$v-$arch" ]; then bundle="$candidate/$v-$arch"; break; fi
    fi
  done
fi
if [ -z "$bundle" ] || [ ! -f "$bundle/run.sh" ]; then
  echo "no bundle found. Pass one: $0 <bundle directory>" >&2
  exit 2
fi

# The version the artifact is named after, and the tag the release is published under.
if [ -f "$bundle/version" ] && [ "$version" = "local" ]; then
  version=$(tr -d '\n' < "$bundle/version")
fi
arch=$(uname -m); case "$arch" in aarch64|arm64) arch=aarch64 ;; *) arch=x86_64 ;; esac
[ -n "$tag" ] || tag="runtime-$version-$arch"

if [ -z "$repo" ]; then
  repo=$(git -C "$root" remote get-url origin 2>/dev/null |
    sed -e 's#^git@[^:]*:##' -e 's#^https://[^/]*/##' -e 's#\.git$##' || true)
fi
[ -n "$repo" ] || { echo "cannot tell which repository to publish to; pass --repo" >&2; exit 2; }

out=$(mktemp -d)
echo "packing $bundle"
"$here/bundle/bundle.sh" pack "$bundle" "$out" --version "$version" >/dev/null
archive="$out/runtime-$version-$arch.tar.xz"
[ -f "$archive" ] || { echo "packing produced no archive" >&2; exit 1; }
ls -lh "$out" | tail -2

command -v gh >/dev/null || { echo "gh is not installed: https://cli.github.com" >&2; exit 2; }

echo
echo "release $tag in $repo:"
echo "  runtime-$version-$arch.tar.xz"
echo "  runtime-$version-$arch.tar.xz.sha256"

if [ "$dry_run" = 1 ]; then
  echo
  echo "dry run: not publishing. Without --dry-run this would run:"
  echo "  gh release create $tag --repo $repo --latest --title ... $archive $archive.sha256"
  exit 0
fi

draft_flag=""
[ "$draft" = 1 ] && draft_flag="--draft"
# shellcheck disable=SC2086
# The name to quote back for this release: a published one is its tag, a draft has no
# tag and GitHub calls it `untagged-<id>`, and printing the tag for a draft gives a
# command that does not work.
release=$(gh release create "$tag" --repo "$repo" --latest $draft_flag \
  --title "Runtime bundle $version ($arch)" \
  --notes "Mosaic runtime bundle for $arch, built from a system image.

Install it on a machine of this architecture with:

    mosaic runtime install https://github.com/$repo/releases/download/$tag/runtime-$version-$arch.tar.xz

The archive holds the image's ART, Bionic and framework jars, unmodified." \
  "$archive" "$archive.sha256" | tail -1)
name=${release##*/}

echo
if [ "$draft" = 1 ]; then
  echo "uploaded as a draft: not visible, nothing public, and removable with"
  echo "  gh release delete $name --repo $repo --cleanup-tag"
  echo
  echo "publish it -- this is the step that makes it visible -- with:"
  echo "  gh release edit $name --repo $repo --draft=false"
  echo
fi
echo "On a machine of this architecture:"
echo "  mosaic runtime install https://github.com/$repo/releases/download/$tag/runtime-$version-$arch.tar.xz"
echo
echo "or, for every such machine, in <work>/mosaic.cfg:"
echo "  bundle_channel = https://github.com/$repo/releases/latest/download"
echo "  bundle_version = $version"

# A private repository is the case where the lines above are not enough, and it is
# worth saying so here rather than letting the 404 explain itself: github.com's
# download URLs are browser-facing and answer 404 to a private repository with or
# without a token. The API asset URLs do serve it, and `mosaic runtime fetch` uses
# them whenever MOSAIC_BUNDLE_TOKEN is set.
if [ "$(gh repo view "$repo" --json isPrivate --jq '.isPrivate' 2>/dev/null)" = "true" ]; then
  echo
  echo "note: $repo is private, so those URLs answer 404 without a token."
  echo "A machine installs from it by setting MOSAIC_BUNDLE_TOKEN to a token that"
  echo "can read the repository; the release is then resolved through the API:"
  echo "  MOSAIC_BUNDLE_TOKEN=<token> mosaic runtime fetch"
  echo "Making the repository public makes the plain URLs work instead."
fi

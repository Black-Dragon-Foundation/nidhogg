#!/usr/bin/env bash
# CI's prebuilt images, so a job pulls its packages instead of installing them (README.md, "CI and releases"):
# ghcr.io/black-dragon-foundation/nidhogg-NAME:TAG, built from nidhogg/ci/images/*.Containerfile.  TAG is a hash of the
# build context, which holds exactly the Containerfile and every file it copies (the inputs below), plus the build
# target and this script, so an image changes its tag whenever anything it is built from changes.
#   image.sh tag NAME          print the tag
#   image.sh resolve NAME      print REF@sha256:DIGEST when the tag is published (nothing, exit 0, when it is not)
#   image.sh publish NAME      build the tag and push it; print REF@sha256:DIGEST
#   image.sh use NAME [REF]    pull REF (a REF@sha256:DIGEST), or with no REF build NAME here (never pushed); print the
#                              image to run
#   image.sh names             print every NAME
#   image.sh outputs NAME...   the images workflow's step (below)
# Logged in to the registry beforehand (podman login; skopeo reads the same auth file).  Exit 0 ok, 1 failed, 2 usage.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
SELF="$ROOT/nidhogg/ci/image.sh"
REG=${NIDHOGG_IMAGE_REGISTRY:-ghcr.io/black-dragon-foundation}
PODMAN=${NIDHOGG_BUILD_PODMAN:-podman}
NAMES="build-fedora-45 build-fedora-45-norust build-ubuntu-26.04 gui-fedora-45 pkg-fedora-45 guiall-fedora-45 check-fedora-45"

usage() { echo "usage: image.sh (tag|resolve|publish|use|local) NAME [REF] | names" >&2; exit 2; }
# NAME: its Containerfile, the stage it builds (empty: the last) and the repository files the Containerfile copies
spec() {
  case $1 in
    build-fedora-45) FILE=build-fedora-45.Containerfile TARGET=full INPUTS="nidhogg/deps/fedora-45.build" ;;
    build-fedora-45-norust) FILE=build-fedora-45.Containerfile TARGET=norust INPUTS="nidhogg/deps/fedora-45.build" ;;
    build-ubuntu-26.04) FILE=build-ubuntu-26.04.Containerfile TARGET="" INPUTS="nidhogg/deps/ubuntu-26.04.build" ;;
    gui-fedora-45) FILE=test-fedora-45.Containerfile TARGET=gui INPUTS="nidhogg/deps/fedora-45.runtime" ;;
    pkg-fedora-45) FILE=test-fedora-45.Containerfile TARGET=pkg INPUTS="nidhogg/deps/fedora-45.runtime" ;;
    guiall-fedora-45) FILE=test-fedora-45.Containerfile TARGET=guiall INPUTS="nidhogg/deps/fedora-45.runtime" ;;
    check-fedora-45) FILE=check-fedora-45.Containerfile TARGET="" INPUTS="nidhogg/deps/fedora-45.check" ;;
    *) echo "image.sh: no image '$1'; one of: $NAMES" >&2; exit 2 ;;
  esac
}
# the build context in a new directory (printed): the Containerfile and the inputs, each under its base name
context() {
  ctx=$(mktemp -d "${TMPDIR:-/tmp}/nh-image.XXXXXX")
  cp "$ROOT/nidhogg/ci/images/$FILE" "$ctx/Containerfile"
  for f in $INPUTS; do cp "$ROOT/$f" "$ctx/"; done
  echo "$ctx"
}
tag_of() {
  ctx=$(context)
  { (cd "$ctx" && find . -type f | LC_ALL=C sort | while IFS= read -r f; do printf '%s %s\n' "$f" "$(sha256sum < "$f" | cut -c1-64)"; done)
    printf 'target %s\nscript %s\n' "$TARGET" "$(sha256sum < "$SELF" | cut -c1-64)"; } | sha256sum | cut -c1-16
  rm -rf "$ctx"
}
build() {   # build REF:TAG from a fresh context (capped as build.sh's package image is)
  ctx=$(context)
  # shellcheck disable=SC2086
  "$PODMAN" build --memory 4g --memory-swap 4g ${TARGET:+--target "$TARGET"} \
    --label "org.opencontainers.image.source=${NIDHOGG_IMAGE_SOURCE:-https://github.com/Black-Dragon-Foundation/nidhogg}" \
    -t "$1" -f "$ctx/Containerfile" "$ctx" >&2 || { rm -rf "$ctx"; echo "image.sh: building $1 failed" >&2; exit 1; }
  rm -rf "$ctx"
}

[ $# -ge 1 ] || usage
cmd=$1; shift
[ "$cmd" != names ] || { echo "$NAMES"; exit 0; }
# outputs NAME...: the images workflow's step.  Logs in with TOKEN as ACTOR, resolves each NAME and, where PUBLISH is
# true (a push to nidhogg-31 or a run by hand), publishes a missing one; prints KEY=REF@DIGEST lines (KEY: NAME with
# '_' for '-' and '.'), an empty REF for an image the jobs must build themselves (never published, or publishing
# failed: no gate rests on it)
if [ "$cmd" = outputs ]; then
  printf '%s' "${TOKEN:?}" | "$PODMAN" login ghcr.io -u "${ACTOR:?}" --password-stdin >&2
  for n in "$@"; do
    ref=$(bash "$SELF" resolve "$n")
    if [ -z "$ref" ] && [ "${PUBLISH:-}" = true ]; then ref=$(bash "$SELF" publish "$n") || ref=""; fi
    echo "image.sh: $n: ${ref:-none (the jobs build it)}" >&2
    k=${n//-/_}; echo "${k//./_}=$ref"
  done
  exit 0
fi
[ $# -ge 1 ] || usage
NAME=$1; spec "$NAME"
REF="$REG/nidhogg-$NAME"
case $cmd in
  tag) tag_of ;;
  resolve)
    tag=$(tag_of)
    # the digest is the sha256 of the manifest as the registry serves it; no manifest, no image
    if raw=$(skopeo inspect --raw "docker://$REF:$tag" 2>/dev/null) && [ -n "$raw" ]; then
      echo "$REF@sha256:$(printf '%s' "$raw" | sha256sum | cut -c1-64)"
    else
      echo "image.sh: $REF:$tag is not published" >&2
    fi ;;
  publish)
    tag=$(tag_of)
    build "$REF:$tag"
    d=$(mktemp)
    "$PODMAN" push --digestfile "$d" "$REF:$tag" >&2 || { rm -f "$d"; echo "image.sh: pushing $REF:$tag failed" >&2; exit 1; }
    echo "$REF@$(cat "$d")"; rm -f "$d" ;;
  use)
    if [ -n "${2:-}" ]; then
      case $2 in "$REF@sha256:"*) ;; *) echo "image.sh: $2 is not $REF by digest" >&2; exit 2 ;; esac
      "$PODMAN" pull -q "$2" >&2 || { echo "image.sh: pulling $2 failed" >&2; exit 1; }
      echo "$2"
    else
      tag=$(tag_of)
      lref="localhost/nidhogg-$NAME:$tag"
      echo "image.sh: no published $NAME for this run: building $lref here (not pushed)" >&2
      "$PODMAN" image exists "$lref" || build "$lref"
      echo "$lref"
    fi ;;
  local)
    # nidhogg-KIND:DISTRO-TAG, built here once per tag (never pushed, never pulled): the scripts' own image, so a run
    # installs no packages.  Prints the image to run on stdout, "image: REF (local|built)" on stderr.
    tag=$(tag_of)
    lref="localhost/nidhogg-${NAME%%-*}:${NAME#*-}-$tag"
    if "$PODMAN" image exists "$lref"; then how=local; else build "$lref"; how=built; fi
    echo "image: $lref ($how)" >&2
    echo "$lref" ;;
  *) usage ;;
esac

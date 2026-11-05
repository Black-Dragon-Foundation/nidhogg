#!/usr/bin/env bash
# A stand-in `podman` for check-image-test.sh (put on PATH as `podman`): it keeps which images exist as marker files in
# $STUB_DIR and logs every call to $STUB_DIR/log, one line each ("build REF", "run ARGS...").  Nothing runs.
set -u
d=${STUB_DIR:?}
mark() { printf '%s/img-%s' "$d" "$(printf %s "$1" | tr '/:' '__')"; }
case ${1:-} in
  image)
    case ${2:-} in
      exists) [ -f "$(mark "$3")" ] ;;
      *) exit 0 ;;
    esac ;;
  build)
    shift
    ref=""
    while [ $# -gt 0 ]; do case $1 in -t) ref=$2; shift 2 ;; *) shift ;; esac; done
    echo "build $ref" >> "$d/log"
    : > "$(mark "$ref")" ;;
  run) shift; echo "run $*" >> "$d/log" ;;
  *) echo "$*" >> "$d/log" ;;
esac

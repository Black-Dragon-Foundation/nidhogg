#!/usr/bin/env bash
# Give every tracked file the commit time of HEAD as its mtime (a checkout stamps files with "now"), so that restored build
# outputs, which build.sh makes newer than every source, are never older than a source.  Run in the checkout.
# bash 3.2-safe (macOS).
set -euo pipefail
ct=$(git log -1 --format=%ct)
stamp=$(date -u -d "@$ct" +%Y%m%d%H%M.%S 2>/dev/null || date -u -r "$ct" +%Y%m%d%H%M.%S)
git ls-files -z | TZ=UTC xargs -0 touch -h -t "$stamp"

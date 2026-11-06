#!/bin/bash
# Glitnir P5 spike: build each probe on its own (common.m + pN-*.m), so one that fails to compile costs only its own
# question.  Binaries in $OUT/bin, compiler output in $OUT/logs/build-*.log.  Exit 1 if any probe failed to build.
set -u
here=$(cd "$(dirname "$0")" && pwd)
out=${OUT:-out}
mkdir -p "$out/bin" "$out/logs"
fail=0
for src in "$here"/p[0-9]-*.m; do
  p=$(basename "$src" .m)
  if clang -fobjc-arc -O2 -g -Wall -Wno-deprecated-declarations -mmacosx-version-min=14.0 \
       -framework AppKit -framework QuartzCore -framework CoreGraphics -framework IOSurface \
       -framework CoreText -framework ImageIO -framework CoreImage -framework ScreenCaptureKit \
       -o "$out/bin/$p" "$here/common.m" "$src" > "$out/logs/build-$p.log" 2>&1; then
    echo "built $p"
    grep -c 'warning:' "$out/logs/build-$p.log" | xargs -I{} echo "  {} warning(s)"
  else
    echo "::error::$p failed to build"
    cat "$out/logs/build-$p.log"
    printf '%s\tbuild\tFAILED (logs/build-%s.log)\n' "$p" "$p" >> "$out/results.tsv"
    fail=1
  fi
done
exit "$fail"

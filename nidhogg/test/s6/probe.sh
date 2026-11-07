#!/usr/bin/env bash
# S6 spike (throwaway, never merged): images without Cairo.  Runs in the macOS job after the NS build; OUT gets the findings.
#   probe.sh BUILD_DIR OUT
set -u
B=$1 OUT=$2; mkdir -p "$OUT"; here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../../.." && pwd)
exec > >(tee "$OUT/probe.log") 2>&1
echo "== Q1: image.c with HAVE_NHWL, no USE_CAIRO, no image libraries"
src=$B/src; obj=$B/obj
[ -d "$src" ] || src=$root; [ -d "$obj/src" ] || obj=$(dirname "$(find "$B" -name config.h -not -path '*/stage/*' | head -n 1)")/..
cfg=$(find "$B" -name config.h | head -n 1); echo "config.h: $cfg"
srcdir=$(dirname "$(find "$B" -path '*/src/image.c' | head -n 1)"); [ -d "$srcdir" ] || srcdir=$root/src
echo "source: $srcdir"
{
  echo '#include <config.h>'
  for d in HAVE_NS HAVE_NSGUI USE_CAIRO HAVE_RSVG HAVE_PNG HAVE_JPEG HAVE_GIF HAVE_TIFF HAVE_WEBP HAVE_XPM HAVE_LCMS2 HAVE_NATIVE_IMAGE_API HAVE_NS_IMAGE; do echo "#undef $d"; done
  echo '#define HAVE_NHWL 1'; echo '#define HAVE_NATIVE_IMAGE_API 1'
  echo "#include \"$srcdir/image.c\""
} > "$srcdir/s6-image-probe.c"
cflags="-I$(dirname "$cfg") -I$srcdir -I$srcdir/../lib -I$(dirname "$cfg")/../lib -DEMACS -D_GNU_SOURCE=1"
echo "compile: clang -fsyntax-only $cflags s6-image-probe.c"
clang -fsyntax-only -ferror-limit=0 $cflags -x objective-c "$srcdir/s6-image-probe.c" 2> "$OUT/q1-errors.txt"; echo "clang exit $?"
grep -c 'error:' "$OUT/q1-errors.txt" | sed 's/^/errors: /'
grep 'error:' "$OUT/q1-errors.txt" | sed -E 's/^[^:]*:([0-9]+):[0-9]+: /\1: /' | sort | uniq -c | sort -rn | head -40
echo "Cairo sites in image.c: $(grep -c 'USE_CAIRO' "$srcdir/image.c")  pgtk sites: $(grep -c 'HAVE_PGTK' "$srcdir/image.c")  cairo_ calls: $(grep -c 'cairo_' "$srcdir/image.c")"
echo "== Q3: configure with every optional image library off"
if [ -x "$root/configure" ]; then
  mkdir -p "$OUT/cfg" && ( cd "$OUT/cfg" && "$root/configure" --with-ns --without-native-compilation --without-rsvg --without-webp --without-png --without-jpeg --without-gif --without-tiff --without-xpm --without-lcms2 --without-gnutls --without-sqlite3 --without-tree-sitter --without-libgmp > configure.log 2>&1; echo "configure exit $?" )
  grep -E 'Does Emacs (use|support) .*(png|jpeg|gif|tiff|webp|librsvg|lcms|imagemagick|ImageIO|native image)' "$OUT/cfg/configure.log" | head -20
  grep -E 'native image|NATIVE_IMAGE' "$OUT/cfg/config.log" "$OUT/cfg/src/config.h" 2>/dev/null | head -5
fi
echo "otool -L of the stock NS build:"; otool -L "$B"/stage/*/Nidhogg.app/Contents/MacOS/Emacs 2>/dev/null | grep -i 'homebrew\|/opt/' | head -20
echo "== Q2: CGImageSource against NSImage on test/data/image"
clang -fobjc-arc -framework AppKit -framework ImageIO -framework CoreGraphics -o "$OUT/imgprobe" "$here/imgprobe.m" && "$OUT/imgprobe" "$root"/test/data/image/* | tee "$OUT/q2.txt"
echo "== Q5: resvg against librsvg on the Lucide set"
bash "$here/svgcmp.sh" "$here/icons" "$OUT/svg"

#!/usr/bin/env bash
# The glue's boundary checks on a built Emacs (nidhogg/build.sh --nidhogg-wayland):
# 1. Rust never calls Emacs: the undefined symbols of libnhglue.a, minus the ones the archive defines itself
#    (references across its members), must not include any symbol that Emacs's own objects (src/*.o) define.  That
#    denylist is built from the build tree each time.  Calls through function pointers are invisible here; there are
#    none, because C passes no callbacks to the glue (a header rule).
# (1b. Sjá's sja_call is defined in the archive; src/nidhogg-views.c calls it directly, outside NHG: it runs inside a
#  DEFUN, never while waiting for input, so no signal handler's longjmp can cross it, and Sjá's guard checks headroom.)
# 2. Every glue call in src/nhwl*.c and src/nhmac*.c goes through the NHG (…) macro (it clears waiting_for_input, so no
#    longjmp crosses a Rust frame): a line with an `nhg_name (` call and no `NHG (` is reported.  A warning until the
#    macro exists.
# 3. The one function pointer C hands the glue, Naglfar's text source, is registered from one place: once
#    nhg_set_text_source is in nhglue.h, src/*.c must call it on exactly one line, in src/nhmacterm.c.  A source rule
#    beside the symbol rule, since function pointers are invisible to nm.
# ELF (Linux) and Mach-O (macOS) alike: nm -P on both, with Mach-O's leading underscore stripped.
# Usage: nm-check.sh BUILD_DIR   (build.sh's --build-dir, which holds obj/src, or a configured tree's top, which holds
# src).  Prints "nm-check: ok", or the offending names and exits 1; 2 on usage or a missing build.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SRC=$(cd "$HERE/../.." && pwd)/src
# the NHG grep's verdict: warn until the NHG macro is in src/nhwlterm.h, fail from then on
NHG_RULE=warn
grep -q '^# *define NHG *(' "$SRC/nhwlterm.h" 2>/dev/null && NHG_RULE=fail
[ $# -eq 1 ] || { echo "usage: nm-check.sh BUILD_DIR" >&2; exit 2; }
B=$1
if [ -d "$B/obj/src" ]; then obj=$B/obj/src; else obj=$B/src; fi
lib=$obj/glitnir-target/release/libnhglue.a
[ -f "$lib" ] || { echo "nm-check: no $lib (build with --nidhogg-wayland first)" >&2; exit 2; }
ls "$obj"/*.o >/dev/null 2>&1 || { echo "nm-check: no objects in $obj" >&2; exit 2; }
command -v nm >/dev/null 2>&1 || { echo "nm-check: nm not found (binutils)" >&2; exit 2; }
bad=0

# POSIX output: "NAME TYPE [VALUE SIZE]"; the archive's member headers ("lib.a[x.o]:") have no type field
strip=cat
[ "$(uname -s)" = Darwin ] && strip="sed s/^_//"
undef=$(nm -P -g "$lib" 2>/dev/null | awk 'NF >= 2 && ($2 == "U" || $2 == "w" || $2 == "v") {print $1}' | $strip | sort -u)
def=$(nm -P -g --defined-only "$lib" 2>/dev/null | awk 'NF >= 2 {print $1}' | $strip | sort -u)
external=$(comm -23 <(printf '%s\n' "$undef") <(printf '%s\n' "$def"))
emacs=$(nm -P -g --defined-only "$obj"/*.o 2>/dev/null | awk 'NF >= 2 && $1 !~ /:$/ {print $1}' | $strip | sort -u)
[ -n "$emacs" ] || { echo "nm-check: no symbols defined by $obj/*.o" >&2; exit 2; }
calls=$(comm -12 <(printf '%s\n' "$external") <(printf '%s\n' "$emacs"))
if [ -n "$calls" ]; then
  echo "nm-check: libnhglue.a references symbols Emacs defines (Rust must never call Emacs):"
  printf '  %s\n' $calls
  bad=1
fi

# Sjá's entry point is in the archive (Glitnir re-exports it; src/nidhogg-views.c calls it), and it too calls nothing
# of Emacs (the check above covers its members, which are the archive's)
if ! printf '%s\n' "$def" | grep -qx sja_call; then
  echo "nm-check: libnhglue.a does not define sja_call (Sjá's entry point; glitnir's pub use sja::abi::sja_call)"
  echo "nm-check: $(printf '%s\n' "$def" | grep -c .) defined names read; nm's lines naming sja_call:"
  nm -P -g "$lib" 2>&1 | grep -F sja_call | head -n 5 | sed 's/^/  /'
  bad=1
fi

# the NHG grep (comments are not exempt: write the name without the parenthesis there)
unwrapped=$(cd "$SRC/.." && grep -n 'nhg_[a-z_]* (' src/nhwl*.c src/nhmac*.c 2>/dev/null | grep -v 'NHG (')
if [ -n "$unwrapped" ]; then
  if [ "$NHG_RULE" = fail ]; then
    echo "nm-check: glue calls outside NHG (…):"; printf '%s\n' "$unwrapped" | sed 's/^/  /'; bad=1
  else
    echo "nm-check: warning: glue calls outside NHG (…) (the macro is not there yet):"
    printf '%s\n' "$unwrapped" | sed 's/^/  /'
  fi
fi

# the text source's one registration
if grep -q 'nhg_set_text_source' "$SRC/../nidhogg/glue/include/nhglue.h"; then
  sites=$(cd "$SRC/.." && grep -n 'nhg_set_text_source (' src/*.c)
  if [ "$(printf '%s\n' "$sites" | grep -c .)" != 1 ] || [ "${sites%%:*}" != src/nhmacterm.c ]; then
    echo "nm-check: nhg_set_text_source must be called on exactly one line, in src/nhmacterm.c; found:"
    printf '%s\n' "${sites:-  (none)}" | sed 's/^/  /'; bad=1
  fi
fi

[ "$bad" = 0 ] || exit 1
echo "nm-check: ok ($(printf '%s\n' "$external" | grep -c .) external references checked against $(printf '%s\n' "$emacs" | grep -c .) Emacs symbols)"

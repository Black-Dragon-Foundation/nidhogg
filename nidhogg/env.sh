# Environment of a relocated Nidhogg tree.  Usage (POSIX sh, sourced):
#   NIDHOGG_ROOT=DIR . DIR/share/nidhogg/env.sh
# Not needed when the tree sits at the prefix recorded in share/nidhogg/BUILDINFO.
R=${NIDHOGG_ROOT:?set NIDHOGG_ROOT to the unpacked tree}
for v in "$R"/share/emacs/[0-9]*; do [ -d "$v" ] && ver=${v##*/} && break; done
for t in "$R"/libexec/emacs/"$ver"/*; do [ -d "$t" ] && triple=${t##*/} && break; done
EMACSDATA=$R/share/emacs/$ver/etc
EMACSDOC=$R/share/emacs/$ver/etc
EMACSPATH=$R/libexec/emacs/$ver/${triple:-}
# prepend to an existing value; with none, the trailing colon keeps Emacs's default load-path
EMACSLOADPATH=$R/share/emacs/$ver/lisp:$R/share/emacs/site-lisp:${EMACSLOADPATH-}
INFOPATH=$R/share/info${INFOPATH:+:$INFOPATH}
export EMACSDATA EMACSDOC EMACSPATH EMACSLOADPATH INFOPATH
unset v t ver triple

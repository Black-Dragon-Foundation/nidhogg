# The macOS app bundle (Naglfar)

`configure --with-nidhogg-macos` builds `nextstep/Emacs.app` from this directory, the way the NS port builds it from
`nextstep/Cocoa`; upstream's files stay untouched.

| File | Copied from (GNU Emacs 31, `nextstep/`) | Changed |
|---|---|---|
| `Info.plist.in` | `templates/Info.plist.in` | no (keeps `NSServices` and `CFBundleDocumentTypes`) |
| `InfoPlist.strings.in` | `templates/InfoPlist.strings.in` | the info string names Nidhogg and Naglfar |
| `Emacs.base/Contents/PkgInfo` | `Cocoa/Emacs.base/Contents/PkgInfo` | no |
| `Emacs.base/Contents/Resources/{Credits.html,Emacs.icns,document.icns}` | `Cocoa/Emacs.base/Contents/Resources/` | no |

`configure` writes `Emacs.base/Contents/Info.plist` and `Emacs.base/Contents/Resources/English.lproj/InfoPlist.strings`
from the two templates; `nextstep/Makefile` assembles the bundle and `make install` fills it (self-contained, as NS).
`install.sh` signs it ad hoc.

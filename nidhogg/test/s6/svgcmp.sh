#!/usr/bin/env bash
# Render each SVG with rsvg-convert (librsvg, brew) and with resvg (cargo, crates.io); compare per icon.  svgcmp.sh DIR OUT
set -u
D=$1 O=$2; mkdir -p "$O/a" "$O/b" "$O/resvg"; here=$(cd "$(dirname "$0")" && pwd)
cat > "$O/resvg/Cargo.toml" <<'T'
[package]
name = "svgcmp"
version = "0.0.0"
edition = "2021"
[dependencies]
resvg = "0.45"
[profile.release]
opt-level = "s"
lto = true
strip = true
[workspace]
T
mkdir -p "$O/resvg/src" && cat > "$O/resvg/src/main.rs" <<'R'
use resvg::{tiny_skia, usvg};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&a[1]).unwrap();
    let size: u32 = a[3].parse().unwrap();
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).unwrap();
    let mut pm = tiny_skia::Pixmap::new(size, size).unwrap();
    let s = size as f32 / tree.size().width();
    resvg::render(&tree, tiny_skia::Transform::from_scale(s, s), &mut pm.as_mut());
    pm.save_png(&a[2]).unwrap();
}
R
( cd "$O/resvg" && cargo build --release 2>&1 | tail -3 )
bin=$O/resvg/target/release/svgcmp; echo "resvg binary: $(stat -f %z "$bin" 2>/dev/null) bytes (stripped, opt-level s, lto)"
echo "librsvg: $(rsvg-convert --version) ; binary size $(stat -f %z "$(command -v rsvg-convert)") bytes + libs: $(otool -L "$(command -v rsvg-convert)" | grep -c rsvg) librsvg dylib"
ls -L "$(brew --prefix librsvg)/lib/"librsvg-2*.dylib 2>/dev/null | head -1 | xargs -I{} stat -f 'librsvg dylib %z bytes' {}
n=0; worst=0
for f in "$D"/*.svg; do
  b=$(basename "$f" .svg)
  rsvg-convert -w 48 -h 48 "$f" -o "$O/a/$b.png" 2>/dev/null || { echo "$b: librsvg failed"; continue; }
  "$bin" "$f" "$O/b/$b.png" 48 2>/dev/null || { echo "$b: resvg failed"; continue; }
  n=$((n+1))
done
python3 - "$O" <<'P'
import sys, os, struct, zlib
O = sys.argv[1]
def png(p):
    d = open(p, 'rb').read(); i = 8; idat = b''; w = h = ct = 0
    while i < len(d):
        l, t = struct.unpack('>I4s', d[i:i+8]); c = d[i+8:i+8+l]
        if t == b'IHDR': w, h, bd, ct = struct.unpack('>IIBB', c[:10])
        if t == b'IDAT': idat += c
        i += 12 + l
    raw = zlib.decompress(idat); bpp = {6: 4, 2: 3, 0: 1, 4: 2}[ct]; s = w * bpp; out = []; prev = bytearray(s); k = 0
    for _ in range(h):
        f = raw[k]; row = bytearray(raw[k+1:k+1+s]); k += 1 + s
        for x in range(s):
            a = row[x-bpp] if x >= bpp else 0; b = prev[x]; c2 = prev[x-bpp] if x >= bpp else 0
            if f == 1: row[x] = (row[x] + a) & 255
            elif f == 2: row[x] = (row[x] + b) & 255
            elif f == 3: row[x] = (row[x] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c2), abs(a - c2), abs(a + b - 2 * c2)
                row[x] = (row[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c2)) & 255
        out.append(row); prev = row
    return b''.join(out), ct
rows = []
for f in sorted(os.listdir(O + '/a')):
    pa = O + '/a/' + f; pb = O + '/b/' + f
    if not os.path.exists(pb): continue
    (x, c1), (y, c2) = png(pa), png(pb)
    if len(x) != len(y) or c1 != c2: rows.append((f, -1, -1)); continue
    d = [abs(p - q) for p, q in zip(x, y)]
    rows.append((f, max(d), sum(d) / len(d)))
open(O + '/svg-diffs.tsv', 'w').write('\n'.join('%s\t%s\t%.3f' % r for r in rows))
good = [r for r in rows if r[1] >= 0]
print('icons compared: %d, size mismatches: %d' % (len(good), len(rows) - len(good)))
if good:
    ms = sorted(r[1] for r in good); mn = sorted(r[2] for r in good)
    print('max channel diff: median %d, p95 %d, worst %d (%s)' % (ms[len(ms)//2], ms[int(len(ms)*.95)], ms[-1], max(good, key=lambda r: r[1])[0]))
    print('mean abs diff: median %.3f, p95 %.3f' % (mn[len(mn)//2], mn[int(len(mn)*.95)]))
    print('icons with max diff > 64: %d' % sum(1 for r in good if r[1] > 64))
P

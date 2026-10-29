#!/usr/bin/env bash
# The budgets matrix: budgets.sh over scales x sizes x configs, one plist file per cell and a summary.
#
#   nidhogg/test/budgets-matrix.sh --prefix-a SHELL --drgnx DIR --out-dir D [--cells LIST] [--runs N] [--resume]
#   nidhogg/test/budgets-matrix.sh --self-test --prefix-a SHELL [--drgnx DIR] --out-dir D [--cells LIST]
#
#   --prefix-a   an unpacked Nidhogg build with --nidhogg-wayland.  Every cell runs budgets.sh on it: side A (presenter
#                thread) and side C (NHWL_PRESENTER=0), A and C alternating in budgets.sh's typing sessions (AC, CA, ...)
#   --drgnx      a drgnx checkout; the "drgnx" cells start both sides with its configuration (budgets.sh --config drgnx)
#   --out-dir    cell-SCALE-WxH-CONFIG.eld per cell (budgets.sh's plists) and summary.md (row x cell: A / C and the
#                verdict against the row's absolute target and :kind)
#   --cells      comma-separated SCALExWIDTHxHEIGHTxCONFIG, CONFIG q or drgnx (default: the 12 cells below)
#   --runs N     sessions per side (default 5)
#   --resume     skip the cells whose .eld exists.  A cell is written to .eld.part and renamed when budgets.sh
#                finishes, so an .eld is always whole
#   --self-test  budgets.sh's own --self-test (1 run, 1280x720, short windows) on --cells (default 1x1280x720xq); then
#                every line of every cell must read back as a plist with :target, :kind and :verdict, and summary.md
#                must have a row for each.  Not part of the matrix proper
#
# The 12 cells: scales 1, 1.25, 1.5 and 2 x 1920x1080 and 3840x2160 on -Q (8), plus drgnx's configuration at
# 1920x1080 x 1, 1920x1080 x 1.25, 3840x2160 x 1.5 and 3840x2160 x 2 (4).  There is no pgtk side and no ratio against
# one: the verdicts are budgets.sh's, on absolute targets.  The one pgtk reference run belongs to the gate.
# BUDGETS_CPUSET pins the containers.  When it is unset and DRGNX_PERF_CPUS=N is, the first N physical cores' first
# threads are used (the island's pinning, lscpu CPU,CORE).  Timing runs go through perf-exclusive.sh.
# Exit 0 when every cell ran, 1 when a cell failed (the others still run; --resume picks up the rest), 2 usage.
set -u

self=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
default_cells=1x1920x1080xq,1x3840x2160xq,1.25x1920x1080xq,1.25x3840x2160xq,1.5x1920x1080xq,1.5x3840x2160xq,2x1920x1080xq,2x3840x2160xq,1x1920x1080xdrgnx,1.25x1920x1080xdrgnx,1.5x3840x2160xdrgnx,2x3840x2160xdrgnx

pa='' drgnx='' outdir='' cells='' runs=5 resume=0 selftest=0
while [ $# -gt 0 ]; do
  case $1 in
    --prefix-a) pa=${2:-}; shift 2 ;;
    --drgnx) drgnx=${2:-}; shift 2 ;;
    --out-dir) outdir=${2:-}; shift 2 ;;
    --cells) cells=${2:-}; shift 2 ;;
    --runs) runs=${2:-}; shift 2 ;;
    --resume) resume=1; shift ;;
    --self-test) selftest=1; shift ;;
    -h|--help) sed -n '2,/^# Exit/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2; exit 2 ;;
    *) echo "budgets-matrix.sh: unknown argument $1" >&2; exit 2 ;;
  esac
done
[ -x "$pa/bin/nidhogg" ] || { echo "budgets-matrix.sh: --prefix-a must be an unpacked Nidhogg" >&2; exit 2; }
[ -n "$outdir" ] || { echo "budgets-matrix.sh: --out-dir is required" >&2; exit 2; }
case $runs in ''|*[!0-9]*|0) echo "budgets-matrix.sh: --runs is a whole number" >&2; exit 2 ;; esac
if [ -z "$cells" ]; then
  if [ "$selftest" = 1 ]; then cells=1x1280x720xq; else cells=$default_cells; fi
fi
case ,$cells, in *xdrgnx,*) [ -f "$drgnx/init/init.el" ] || { echo "budgets-matrix.sh: a drgnx cell needs --drgnx DIR, a drgnx checkout" >&2; exit 2; } ;; esac
mkdir -p "$outdir" || exit 2
outdir=$(cd "$outdir" && pwd)

# the island's pinning, as the harness does it: one thread per physical core
if [ -z "${BUDGETS_CPUSET:-}" ] && [ -n "${DRGNX_PERF_CPUS:-}" ]; then
  BUDGETS_CPUSET=$(lscpu -p=CPU,CORE | awk -F, -v n="$DRGNX_PERF_CPUS" '/^#/ {next} !($2 in s) && k < n {s[$2] = 1; printf "%s%s", (k++ ? "," : ""), $1}')
  export BUDGETS_CPUSET
fi

# cell_name SCALExWxHxCONFIG: sets c_scale c_size c_config c_file, or fails
cell_name() {
  local w h
  IFS=x read -r c_scale w h c_config <<< "$1"
  case $c_scale$w$h in ''|*[!0-9.]*) return 1 ;; esac
  [ -n "$c_scale" ] && [ -n "$w" ] && [ -n "$h" ] || return 1
  case $c_config in q|drgnx) ;; *) return 1 ;; esac
  c_size=${w}x$h
  c_file=$outdir/cell-$c_scale-$c_size-$c_config.eld
}

failed=0 files=()
IFS=, read -r -a cell_list <<< "$cells"
for cell in "${cell_list[@]}"; do
  cell_name "$cell" || { echo "budgets-matrix.sh: bad cell $cell (SCALExWIDTHxHEIGHTxq|drgnx)" >&2; exit 2; }
done
for cell in "${cell_list[@]}"; do
  cell_name "$cell"
  files+=("$c_file")
  if [ "$resume" = 1 ] && [ -s "$c_file" ]; then echo "budgets-matrix.sh: $cell is done, skipped"; continue; fi
  echo "budgets-matrix.sh: $cell"
  bargs=(--prefix-a "$pa" --scale "$c_scale" --size "$c_size" --runs "$runs" --config "$c_config" --out "$c_file.part")
  [ "$c_config" != drgnx ] || bargs+=(--drgnx "$drgnx")
  [ "$selftest" = 0 ] || bargs+=(--self-test)
  if bash "$self/budgets.sh" "${bargs[@]}" > "$outdir/cell-$c_scale-$c_size-$c_config.log" 2>&1 && [ -s "$c_file.part" ]; then
    mv "$c_file.part" "$c_file"
  else
    echo "budgets-matrix.sh: $cell failed (see ${c_file%.eld}.log)" >&2
    failed=1
  fi
done

# summary.md: one line per row, one column per cell: "A / C verdict" (C only where the row has a C side).  The kind and
# target columns are those of the strongest cell the row has (gate, budget, report); a row's target can differ between cells
present=()
for f in "${files[@]}"; do [ ! -s "$f" ] || present+=("$f"); done
if [ "${#present[@]}" -gt 0 ]; then
  awk '
    function field(line, re,   m) { return match(line, re) ? substr(line, RSTART, RLENGTH) : "" }
    FNR == 1 {
      cell = FILENAME; sub(/.*\/cell-/, "", cell); sub(/\.eld$/, "", cell); ncell++; cells[ncell] = cell
    }
    /^\(:row / {
      row = $2
      if (!(row in seen)) { seen[row] = 1; order[++nrows] = row }
      a = field($0, ":a \\([^ )]+");   sub(/^:a \(/, "", a)
      c = field($0, ":c \\([^ )]+");   sub(/^:c \(/, "", c)
      v = field($0, ":verdict [a-z]+\\)$"); sub(/^:verdict /, "", v); sub(/\)$/, "", v)
      k = field($0, ":kind [a-z]+");  sub(/^:kind /, "", k)
      t = field($0, ":target [^ ]+"); sub(/^:target /, "", t)
      cellv[row, ncell] = (a == "" ? "-" : a) (c == "" ? "" : " / " c) " " v
      rank = (k == "gate") ? 3 : (k == "budget") ? 2 : 1
      if (rank > best[row]) { best[row] = rank; kind[row] = k; target[row] = t }
    }
    END {
      printf "| row | kind | target |"; for (i = 1; i <= ncell; i++) printf " %s |", cells[i]; printf "\n"
      printf "|---|---|---|"; for (i = 1; i <= ncell; i++) printf "---|"; printf "\n"
      for (r = 1; r <= nrows; r++) {
        row = order[r]; printf "| %s | %s | %s |", row, kind[row], target[row]
        for (i = 1; i <= ncell; i++) printf " %s |", ((row, i) in cellv) ? cellv[row, i] : "-"
        printf "\n"
      }
    }' "${present[@]}" > "$outdir/summary.md"
fi

if [ "$selftest" = 1 ]; then
  [ "$failed" = 0 ] || { echo "budgets-matrix self-test: a cell failed" >&2; exit 1; }
  bad=$(awk '!/^\(:row [^ ]+ / || !/ :target / || !/ :kind (gate|budget|report) / || !/ :verdict (pass|fail|report|skip)\)$/ {print FILENAME ": " $0}' "${files[@]}")
  [ -z "$bad" ] || { printf 'budgets-matrix self-test: lines that are not plists:\n%s\n' "$bad" >&2; exit 1; }
  rows=0
  for f in "${files[@]}"; do
    while read -r row; do
      grep -q "^| $row |" "$outdir/summary.md" || { echo "budgets-matrix self-test: no $row row in summary.md" >&2; exit 1; }
      rows=$((rows + 1))
    done < <(awk '/^\(:row / {print $2}' "$f")
  done
  echo "budgets-matrix self-test: ${#files[@]} cell(s), $rows rows read back and in summary.md"
fi
exit "$failed"

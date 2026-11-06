#!/bin/bash
# Glitnir P5 spike: the results as a Markdown table (stdout, for the job summary) and $OUT/index.html, every
# screenshot with its name, grouped by probe, for picking the shadow.
set -u
out=${OUT:-out}
res="$out/results.tsv"
[ -f "$res" ] || { echo "no results"; exit 0; }
esc() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
{
  echo "## Glitnir P5 spike"
  echo
  echo "| probe | key | value |"
  echo "|---|---|---|"
  awk -F '\t' '{ gsub(/\|/, "\\|"); printf "| %s | %s | %s |\n", $1, $2, $3 }' "$res"
}
{
  echo '<!doctype html><meta charset="utf-8"><title>Glitnir P5 spike</title>'
  echo '<style>body{font:14px system-ui;margin:16px;background:#888}figure{display:inline-block;margin:8px;'
  echo 'vertical-align:top}img{max-width:1200px;border:1px solid #444}figcaption{font:12px ui-monospace}'
  echo 'table{border-collapse:collapse;background:#fff}td{border:1px solid #ccc;padding:2px 6px;font:12px ui-monospace}</style>'
  for g in p1 p2 p3 p4 p5 p6; do
    echo "<h2>$g</h2>"
    for f in "$out"/shots/"$g"-*.png; do
      [ -e "$f" ] || continue
      n=$(basename "$f")
      echo "<figure><img src=\"shots/$n\" loading=lazy><figcaption>$n</figcaption></figure>"
    done
  done
  echo '<h2>results</h2><table>'
  esc < "$res" | awk -F '\t' '{ printf "<tr><td>%s</td><td>%s</td><td>%s</td></tr>\n", $1, $2, $3 }'
  echo '</table>'
} > "$out/index.html"

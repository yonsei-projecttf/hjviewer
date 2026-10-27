#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# What prepush runs (scripts/ci.sh prepush) took, added up from their log folders
# (<CI directory>/prepush/<time>-<commit>/, docs/ci.md): for each stage (and job beside a stage, as
# e2e-build) its logs, how many ended with exit 0, how many of those reused a pass instead of running
# ("== CI reused: ..."), and their time in all, by each log's START and EXIT lines (a log without an
# EXIT line, of a run cut off or still running, counts without its time). Since the jobs of a run go
# side by side, those times add up to more than the run took: its own time is the line "prepush:
# passed, <seconds> s" of its summary.txt, added up last. Then the steps that took the most time in
# all, by the folders' times.tsv (which runs before 2026-10-27 do not have).
#   scripts/ci-times.sh [<prepush log folder>...]   every folder of the CI directory (HJV_CI_DIR,
#                                                   default build/ci) when none is named
#   scripts/ci-times.sh build/ci/prepush/20261026-*   the runs of a day
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
CI="${HJV_CI_DIR:-$ROOT/build/ci}"
STAGES="leak server client deploy e2e-build drill-build e2e drill wol"
if [ $# -eq 0 ]; then
  [ -d "$CI/prepush" ] || { echo "ci-times: no prepush logs in $CI/prepush" >&2; exit 1; }
  set -- "$CI"/prepush/*/
fi
runs="" tsv=() folders=0 logs=0 cut=0 passed=0 wall=0
for dir in "$@"; do
  [ -d "$dir" ] || { echo "ci-times: $dir is no folder" >&2; exit 1; }
  folders=$((folders + 1))
  [ ! -f "$dir/times.tsv" ] || tsv+=("$dir/times.tsv")
  # (the folder of a run before 2026-10-27 has no summary.txt)
  secs=$(sed -n 's/^prepush: passed, \([0-9]*\) s.*/\1/p' "$dir/summary.txt" 2> /dev/null | tail -n 1 || true)
  if [ -n "$secs" ]; then passed=$((passed + 1)) wall=$((wall + secs)); fi
  for log in "$dir"/*.log; do
    # a stage's log starts with its head; the deploy stage's side logs (deploy-build.log ...) do not
    start=$(sed -n 's/^START //p' "$log" 2> /dev/null | head -n 1)
    [ -n "$start" ] || continue
    logs=$((logs + 1))
    read -r code end < <(sed -n 's/^EXIT //p' "$log" | tail -n 1) || true
    reused=$(grep -c '^== CI reused: ' "$log" || true)
    if [ -n "${end:-}" ]; then
      runs+="$(basename "$log" .log)	$code	$(($(date -d "$end" +%s) - $(date -d "$start" +%s)))	$reused"$'\n'
    else
      runs+="$(basename "$log" .log)	-		$reused"$'\n'
      cut=$((cut + 1))
    fi
    code="" end=""
  done
done
echo "$folders folders, $logs stage logs ($cut without an EXIT line)"
printf '%s' "$runs" | awk -F'\t' -v order="$STAGES" '
  { n[$1]++; all++; if ($2 == "0") { ok[$1]++; allok++ } if ($4 > 0) { re[$1]++; allre++ } s[$1] += $3; t += $3 }
  function row(name, count, good, reused, secs) {
    printf "%-11s %5d %7d %7d %9d %8.1f\n", name, count, good, reused, secs, secs / 60
  }
  END {
    printf "%-11s %5s %7s %7s %9s %8s\n", "stage", "logs", "exit 0", "reused", "seconds", "minutes"
    k = split(order, st, " ")
    for (i = 1; i <= k; i++) { known[st[i]] = 1; if (n[st[i]]) row(st[i], n[st[i]], ok[st[i]], re[st[i]], s[st[i]]) }
    for (name in n) if (!(name in known)) row(name, n[name], ok[name], re[name], s[name])
    row("all", all, allok, allre, t)
  }'
echo "the $passed runs that passed took $wall s ($(awk -v s="$wall" 'BEGIN { printf "%.1f", s / 60 }') min) from start to end"
[ ${#tsv[@]} -gt 0 ] || exit 0
echo
echo "the steps that took the most time in all ($(printf '%s\n' "${tsv[@]}" | wc -l) folders with times.tsv):"
cat "${tsv[@]}" | awk -F'\t' '{ s[$1 "\t" $3] += $2; n[$1 "\t" $3]++ } END { for (k in s) printf "%d\t%d\t%s\n", s[k], n[k], k }' |
  sort -t "$(printf '\t')" -k1,1nr -k3 | head -n 20 |
  awk -F'\t' '{ step = $4; if (index(step, $3) != 1) step = $3 ": " step; printf "%8d s %4d runs  %s\n", $1, $2, step }'

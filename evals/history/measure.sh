#!/usr/bin/env bash
# Times `graff history` on a repository's history: the whole of it, three
# times on a new cache each, then its last ten commits mapped one at a time
# from a clone moved a commit forward each time. The configuration first.
#
#   evals/history/measure.sh GRAFF REPOSITORY WORK > results.txt
#
# REPOSITORY is cloned into WORK, which is emptied first; it is left alone.
set -euo pipefail
graff=$(realpath "$1")
repository=$2
work=$3
echo "# $(date -Is) graff $graff ($(sha1sum < "$graff" | cut -c1-12)), $(git --version)"
echo "# load: $(cut -d' ' -f1-3 /proc/loadavg); power profile: $(cat /sys/firmware/acpi/platform_profile 2>/dev/null || echo unknown); on AC: $(cat /sys/class/power_supply/AC*/online 2>/dev/null || echo unknown); $(nproc) threads"
rm -rf "$work"
mkdir -p "$work"
git clone -q "$repository" "$work/clone"
tip=$(git -C "$work/clone" rev-parse HEAD)
echo "# $repository at $tip: $(git -C "$work/clone" rev-list --count --no-merges HEAD) commits but merges"
for round in 1 2 3; do
  export XDG_CACHE_HOME=$work/cache-whole-$round
  echo "whole, round $round: $(command time -f '%e s real, %U s user, %S s sys, %M KB' "$graff" history "$work/clone" 2>&1 | sed 's/^[^:]*: //' | tr '\n' ' ')"
done
export XDG_CACHE_HOME=$work/cache-new
git -C "$work/clone" checkout -q --detach "$tip~10"
"$graff" history "$work/clone" > /dev/null
for n in $(seq 9 -1 0); do
  git -C "$work/clone" checkout -q --detach "$tip~$n"
  echo "new commit $(git -C "$work/clone" rev-parse --short HEAD): $("$graff" history "$work/clone" | sed 's/^[^:]*: //')"
done

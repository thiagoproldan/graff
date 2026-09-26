"""How many paired runs the verdict needs (task 4), from the noise measured so
far. Re-run it after the pilot: the A/A pairs it adds replace the estimates.

Noise is the standard deviation of the paired log-difference of units per task
between two cells, sigma_d. Measured: every task and cell that ekko's paired
harness ran twice (A/A: the same cell, so the true difference is zero and
sigma_d is sqrt(mean(d^2))). Borrowed: JetBrains' rtk study, 80 pairs on
SkillsBench with Sonnet 5 at low effort, a median +7.6% at p=0.004 (Wilcoxon),
which puts sigma_d near 0.22 (by the t approximation: 0.073 * sqrt(80) / 2.94).

Pairs needed for a one-sided test at ALPHA with POWER, by the normal
approximation divided by the Wilcoxon test's efficiency against the t test
(0.955 for normal data):

    n = ((z_alpha + z_beta) * sigma_d / delta)^2 / 0.955

    python3 evals/verdict/power.py [--runs ~/Projetos/ekko/target/evals/paired/runs]
"""

import argparse
import collections
import datetime
import glob
import math
import os
import re
import statistics
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "ceiling"))
import ceiling  # noqa: E402

ALPHA, POWER, ARE = 0.05, 0.80, 0.955
RTK_SIGMA = 0.073 * math.sqrt(80) / 2.94
# Units a point of each account's 5-hour window (ekko's evals/paired/harness.py,
# POINTS, measured 2026-09-24 and 26).
POINTS = {"default": 63_000, "trabalho": 77_000}
RUN = re.compile(r"^(?P<task>[^-]+)-(?P<cell>[a-z]+)-(?P<rep>\d+)-")


def measure(root):
    """Units and navigation units of every run under root, by (task, cell)."""
    since = datetime.datetime(2000, 1, 1, tzinfo=ceiling.LOCAL)
    until = datetime.datetime(3000, 1, 1, tzinfo=ceiling.LOCAL)
    cells = collections.defaultdict(list)
    for run in sorted(os.listdir(root)):
        match = RUN.match(run)
        if not match or match["task"] == "probe":
            continue
        paths = [("run", p) for p in sorted(glob.glob(os.path.join(root, run, "transcripts", "**", "*.jsonl"), recursive=True))]
        report = ceiling.analyse(*ceiling.scan(since, until, paths), {})
        cells[(match["task"], match["cell"])].append((run, report["cost"], report["nav_cost"]))
    return cells


def pairs_needed(sigma, delta):
    z = statistics.NormalDist().inv_cdf
    return math.ceil(((z(1 - ALPHA) + z(POWER)) * sigma / delta) ** 2 / ARE)


def detectable(sigma, n):
    z = statistics.NormalDist().inv_cdf
    return (z(1 - ALPHA) + z(POWER)) * sigma / math.sqrt(n * ARE)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", default=os.path.expanduser("~/Projetos/ekko/target/evals/paired/runs"))
    opts = parser.parse_args()
    cells = measure(opts.runs)

    print(f"alpha {ALPHA} one-sided, power {POWER}, Wilcoxon efficiency {ARE}; runs from {opts.runs}")
    print("-- runs")
    short = []
    for (task, cell), runs in sorted(cells.items()):
        for run, cost, nav in runs:
            print(f"  {run:28} {cost / 1e6:5.2f}M, navigation {nav / 1e6:5.2f}M ({nav / cost:4.0%})")
            if cell == "cmax" and cost < 3e6:
                short.append(cost)
    aa = {key: runs for key, runs in cells.items() if len(runs) > 1}
    d_total = [math.log(a[1] / b[1]) for runs in aa.values() for a, b in zip(runs, runs[1:])]
    d_nav = [math.log(a[2] / b[2]) for runs in aa.values() for a, b in zip(runs, runs[1:])]
    sigmas = {"rtk (JetBrains)": RTK_SIGMA}
    print("-- A/A pairs (the same task and cell twice)")
    if d_total:
        s_total = math.sqrt(statistics.fmean(d * d for d in d_total))
        s_nav = math.sqrt(statistics.fmean(d * d for d in d_nav))
        print(f"  {len(d_total)} pair(s) {sorted(aa)}: sigma_d total {s_total:.2f}, navigation {s_nav:.2f}")
        sigmas[f"A/A total ({len(d_total)})"] = s_total
        sigmas[f"A/A navigation ({len(d_nav)})"] = s_nav
    else:
        print("  none")

    print("-- pairs needed, by the effect to detect (a cut of X in units per task)")
    cuts = (0.10, 0.15, 0.20, 0.30)
    print("  " + " " * 24 + "".join(f"{c:>8.0%}" for c in cuts))
    for name, sigma in sigmas.items():
        print(f"  {name:18} {sigma:.2f}  " + "".join(f"{pairs_needed(sigma, -math.log(1 - c)):>8}" for c in cuts))

    print("-- the smallest cut detectable, by pairs")
    sizes = (12, 24, 40)
    print("  " + " " * 24 + "".join(f"{n:>8}" for n in sizes))
    for name, sigma in sigmas.items():
        print(f"  {name:18} {sigma:.2f}  " + "".join(f"{1 - math.exp(-detectable(sigma, n)):>8.0%}" for n in sizes))

    if short:
        pair = 2 * statistics.fmean(short)
        print(f"-- cost: a short task at max took {statistics.fmean(short) / 1e6:.2f}M a run ({len(short)} runs), so a pair ~{pair / 1e6:.1f}M")
        for n in sizes:
            points = "  ".join(f"{account} {n * pair / unit:.0f} points" for account, unit in POINTS.items())
            print(f"  {n} pairs: {n * pair / 1e6:.0f}M units; {points}")


if __name__ == "__main__":
    sys.exit(main())

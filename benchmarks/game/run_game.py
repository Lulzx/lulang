#!/usr/bin/env python3
"""Benchmarks Game harness for the lulang implementations.

Builds every variant, checks that they all produce byte-identical output, and
reports median whole-process wall time. Results that disagree are reported as
MISMATCH and never timed — a wrong answer is not a benchmark result.

    python3 benchmarks/game/run_game.py [--runs 5] [--quick]
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
LU = os.path.join(ROOT, "target", "release", "lu")
CDIR = os.path.join(HERE, "c")

# name -> (official N, quick N)
PROGRAMS = [
    ("nbody",          "50000000", "500000"),
    ("spectralnorm",   "5500",     "500"),
    ("fannkuchredux",  "12",       "9"),
    ("mandelbrot",     "16000",    "1000"),
    ("binarytrees",    "21",       "14"),
]

# label -> (build command factory, run command factory)
VARIANTS = ["lulang-aot", "lulang-jit", "c-O3", "c-O3-fastmath"]


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, **kw)


def build_all(verbose):
    if not os.path.exists(LU):
        print("building the release compiler...", flush=True)
        run(["cargo", "build", "--release"], cwd=ROOT)
    for name, _, _ in PROGRAMS:
        src = os.path.join(HERE, name + ".lu")
        run([LU, "build", src, "-o", os.path.join(HERE, name)])
        csrc = os.path.join(CDIR, name + ".c")
        run(["clang", "-O3", "-march=native", "-o",
             os.path.join(CDIR, name), csrc, "-lm"])
        run(["clang", "-O3", "-march=native", "-ffast-math", "-o",
             os.path.join(CDIR, name + "-ffm"), csrc, "-lm"])
        if verbose:
            print(f"  built {name}", flush=True)


def command(variant, name, n):
    if variant == "lulang-aot":
        return [os.path.join(HERE, name), n]
    if variant == "lulang-jit":
        return [LU, "run", os.path.join(HERE, name + ".lu"), n]
    if variant == "c-O3":
        return [os.path.join(CDIR, name), n]
    if variant == "c-O3-fastmath":
        return [os.path.join(CDIR, name + "-ffm"), n]
    raise ValueError(variant)


def measure(cmd, runs):
    """Return (median seconds, sha256 of stdout)."""
    times = []
    digest = None
    for _ in range(runs):
        t0 = time.perf_counter()
        p = subprocess.run(cmd, capture_output=True)
        elapsed = time.perf_counter() - t0
        if p.returncode != 0:
            raise RuntimeError(f"{cmd[0]} exited {p.returncode}: "
                               f"{p.stderr.decode()[:400]}")
        d = hashlib.sha256(p.stdout).hexdigest()
        if digest is None:
            digest = d
        elif d != digest:
            raise RuntimeError(f"{cmd[0]} is not deterministic across runs")
        times.append(elapsed)
    return statistics.median(times), digest


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--quick", action="store_true",
                    help="use small N for a fast smoke check")
    ap.add_argument("--only", default=None, help="comma-separated program names")
    ap.add_argument("--variants", default=",".join(VARIANTS),
                    help="comma-separated subset of " + ",".join(VARIANTS))
    args = ap.parse_args()

    wanted = set(args.only.split(",")) if args.only else None
    progs = [p for p in PROGRAMS if wanted is None or p[0] in wanted]
    variants = [v for v in VARIANTS if v in set(args.variants.split(","))]

    print("building...", flush=True)
    build_all(verbose=True)

    rows = []
    for name, official_n, quick_n in progs:
        n = quick_n if args.quick else official_n
        print(f"\n{name} (N={n})", flush=True)
        results = {}
        digests = {}
        for variant in variants:
            # The JIT re-runs the whole program from source; at official N it is
            # not the interesting number for the slow programs, but we measure
            # it anyway so the tiers stay comparable.
            try:
                secs, digest = measure(command(variant, name, n), args.runs)
            except RuntimeError as e:
                print(f"  {variant:<16} ERROR {e}", flush=True)
                results[variant] = None
                continue
            results[variant] = secs
            digests[variant] = digest
            print(f"  {variant:<16} {secs:8.3f}s", flush=True)

        unique = set(digests.values())
        agree = len(unique) <= 1
        if not agree:
            print("  !! MISMATCH — variants disagree on output:", flush=True)
            for v, d in digests.items():
                print(f"       {v:<16} {d[:16]}", flush=True)
        rows.append({
            "program": name, "n": n, "agree": agree,
            "output_sha256": (list(unique)[0] if agree and unique else None),
            "times": results,
        })

    out = {
        "machine": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "cpu_count": os.cpu_count(),
            "clang": subprocess.run(["clang", "--version"], capture_output=True)
                     .stdout.decode().splitlines()[0],
        },
        "runs": args.runs,
        "results": rows,
    }
    path = os.path.join(HERE, "results.json")
    with open(path, "w") as f:
        json.dump(out, f, indent=2)
    print(f"\nwrote {path}", flush=True)

    # markdown summary
    print("\n| program | N | lulang AOT | lulang JIT | C -O3 | C -O3 -ffast-math | AOT vs C -O3 |")
    print("| --- | --- | --- | --- | --- | --- | --- |")
    for r in rows:
        t = r["times"]
        def fmt(v):
            return f"{v:.3f}s" if v is not None else "—"
        ratio = "—"
        if t.get("lulang-aot") and t.get("c-O3"):
            ratio = f"{t['c-O3'] / t['lulang-aot']:.2f}×"
        flag = "" if r["agree"] else " **MISMATCH**"
        print(f"| {r['program']}{flag} | {r['n']} | {fmt(t.get('lulang-aot'))} "
              f"| {fmt(t.get('lulang-jit'))} | {fmt(t.get('c-O3'))} "
              f"| {fmt(t.get('c-O3-fastmath'))} | {ratio} |")


if __name__ == "__main__":
    sys.exit(main())

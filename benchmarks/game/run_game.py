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

# name -> (official N, quick N). `needs_input` programs take a FASTA file
# produced by our own fasta at that N, exactly as the benchmark specifies;
# generating it is not timed.
PROGRAMS = [
    ("nbody",          "50000000", "500000"),
    ("spectralnorm",   "5500",     "500"),
    ("fannkuchredux",  "12",       "9"),
    ("mandelbrot",     "16000",    "1000"),
    ("binarytrees",    "21",       "14"),
    ("fasta",          "25000000", "100000"),
    ("revcomp",        "25000000", "100000"),
    ("knucleotide",    "2500000",  "50000"),
]

# These consume a FASTA file rather than a numeric argument.
NEEDS_INPUT = {"revcomp", "knucleotide"}

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


def fasta_input(n):
    """Generate (and cache) the FASTA file the revcomp/knucleotide runs read."""
    path = os.path.join(HERE, f"input-{n}.fasta")
    if not os.path.exists(path):
        print(f"  generating {os.path.basename(path)} ...", flush=True)
        with open(path, "wb") as f:
            subprocess.run([os.path.join(HERE, "fasta"), n], stdout=f, check=True)
    return path


def command(variant, name, n):
    arg = fasta_input(n) if name in NEEDS_INPUT else n
    if variant == "lulang-aot":
        return [os.path.join(HERE, name), arg]
    if variant == "lulang-jit":
        return [LU, "run", os.path.join(HERE, name + ".lu"), arg]
    if variant == "c-O3":
        return [os.path.join(CDIR, name), arg]
    if variant == "c-O3-fastmath":
        return [os.path.join(CDIR, name + "-ffm"), arg]
    raise ValueError(variant)


def binary_id(path):
    """Content hash of a built binary, so a cached baseline invalidates when
    the binary changes."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()[:16]


def load_baselines():
    path = os.path.join(HERE, "baselines.json")
    if os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    return {}


def save_baselines(cache):
    with open(os.path.join(HERE, "baselines.json"), "w") as f:
        json.dump(cache, f, indent=2, sort_keys=True)


def time_once(cmd):
    """Run cmd once; return (seconds, sha256 of stdout)."""
    t0 = time.perf_counter()
    p = subprocess.run(cmd, capture_output=True)
    elapsed = time.perf_counter() - t0
    if p.returncode != 0:
        raise RuntimeError(f"{cmd[0]} exited {p.returncode}: "
                           f"{p.stderr.decode()[:400]}")
    return elapsed, hashlib.sha256(p.stdout).hexdigest()


def measure_interleaved(commands, runs):
    """Time several commands round-robin: run 1 of each, then run 2 of each...

    Running one variant's repeats back-to-back and then the next variant's lets
    a thermal ramp or a background process land entirely on whichever variant
    went first. That is not hypothetical -- it made fannkuch-redux read 0.92x
    when an interleaved re-measure of the same binaries said 0.99x. Round-robin
    spreads any drift evenly across variants, which is the whole point of
    measuring the baseline in the same session.
    """
    times = {name: [] for name in commands}
    digests = {}
    errors = {}
    for _ in range(runs):
        for name, cmd in commands.items():
            if name in errors:
                continue
            try:
                elapsed, digest = time_once(cmd)
            except RuntimeError as e:
                errors[name] = str(e)
                continue
            if name in digests and digests[name] != digest:
                errors[name] = f"{cmd[0]} is not deterministic across runs"
                continue
            digests[name] = digest
            times[name].append(elapsed)
    return (
        {n: statistics.median(v) for n, v in times.items() if v and n not in errors},
        {n: d for n, d in digests.items() if n not in errors},
        errors,
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--quick", action="store_true",
                    help="use small N for a fast smoke check")
    ap.add_argument("--only", default=None, help="comma-separated program names")
    ap.add_argument("--variants", default=",".join(VARIANTS),
                    help="comma-separated subset of " + ",".join(VARIANTS))
    ap.add_argument("--refresh-baseline", action="store_true",
                    help="re-time the C variants even if a cached result for "
                         "the identical binary exists")
    args = ap.parse_args()

    wanted = set(args.only.split(",")) if args.only else None
    progs = [p for p in PROGRAMS if wanted is None or p[0] in wanted]
    variants = [v for v in VARIANTS if v in set(args.variants.split(","))]

    print("building...", flush=True)
    build_all(verbose=True)

    # The C twins do not change between lulang iterations, so their timings are
    # cached by (program, N, variant, binary hash) and reused. The hash means a
    # recompiled or edited twin re-times itself automatically. Pass
    # --refresh-baseline to force it, which is worth doing whenever the machine
    # state may have shifted -- the C column is what makes a lulang delta
    # readable as signal rather than drift.
    baselines = load_baselines()

    rows = []
    for name, official_n, quick_n in progs:
        n = quick_n if args.quick else official_n
        print(f"\n{name} (N={n})", flush=True)
        results = {}
        digests = {}
        to_run = {}
        keys = {}
        for variant in variants:
            cmd = command(variant, name, n)
            if variant.startswith("c-"):
                key = f"{name}:{n}:{variant}:{binary_id(cmd[0])}"
                keys[variant] = key
                cached = baselines.get(key)
                if cached and not args.refresh_baseline:
                    results[variant] = cached["seconds"]
                    digests[variant] = cached["sha256"]
                    print(f"  {variant:<16} {cached['seconds']:8.3f}s  (cached)",
                          flush=True)
                    continue
            to_run[variant] = cmd

        measured, run_digests, errors = measure_interleaved(to_run, args.runs)
        for variant in to_run:
            if variant in errors:
                print(f"  {variant:<16} ERROR {errors[variant]}", flush=True)
                results[variant] = None
                continue
            secs = measured[variant]
            results[variant] = secs
            digests[variant] = run_digests[variant]
            if variant in keys:
                baselines[keys[variant]] = {
                    "seconds": secs, "sha256": run_digests[variant],
                    "runs": args.runs,
                }
                save_baselines(baselines)
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

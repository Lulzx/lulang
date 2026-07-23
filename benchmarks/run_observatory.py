#!/usr/bin/env python3
"""Build and measure the public, source-linked lulang benchmark matrix."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import pathlib
import platform
import shutil
import statistics
import subprocess
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
BUILD = ROOT / "target" / "observatory"


def run(command: list[str], *, capture: bool = False) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    )
    return result.stdout.strip() if capture else ""


def available(program: str) -> str | None:
    return shutil.which(program)


def version(command: list[str]) -> str | None:
    if not available(command[0]):
        return None
    result = subprocess.run(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    return result.stdout.splitlines()[0] if result.stdout else None


def measure(command: list[str], runs: int) -> tuple[float, float]:
    output = run(command, capture=True)
    try:
        answer = float(output.rsplit(":", 1)[1].strip())
    except (IndexError, ValueError) as error:
        raise RuntimeError(f"cannot read benchmark result from {output!r}") from error
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run(
            command,
            cwd=ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        samples.append((time.perf_counter() - start) * 1000.0)
    return statistics.median(samples), answer


def compile_kernel(name: str) -> dict[str, list[str]]:
    source = ROOT / "corpus" / f"bench_{name}"
    host = BUILD / f"{name}-lulang-aot"
    selfhost = BUILD / f"{name}-lulang-selfhost"
    cpp = BUILD / f"{name}-cpp-o3"
    cpp_fast = BUILD / f"{name}-cpp-fast"
    rust = BUILD / f"{name}-rust"

    run(
        [
            str(ROOT / "target/release/lu"),
            "build",
            "-o",
            str(host),
            str(source.with_suffix(".lu")),
        ]
    )
    commands: dict[str, list[str]] = {
        "lulang_aot_ms": [str(host)],
        "lulang_jit_ms": [
            str(ROOT / "target/release/lu"),
            "run",
            str(source.with_suffix(".lu")),
        ],
    }
    if (ROOT / "target/release/luc").exists():
        run(
            [
                str(ROOT / "selfhost/build.sh"),
                str(source.with_suffix(".lu")),
                "-o",
                str(selfhost),
            ]
        )
        commands["lulang_selfhost_ms"] = [str(selfhost)]

    run(
        [
            "clang++",
            "-O3",
            "-mcpu=native",
            str(source.with_suffix(".cpp")),
            "-o",
            str(cpp),
        ]
    )
    run(
        [
            "clang++",
            "-O3",
            "-ffast-math",
            "-mcpu=native",
            str(source.with_suffix(".cpp")),
            "-o",
            str(cpp_fast),
        ]
    )
    commands["cpp_o3_ms"] = [str(cpp)]
    commands["cpp_fast_ms"] = [str(cpp_fast)]

    if available("rustc"):
        run(
            [
                "rustc",
                "-O",
                "-C",
                "target-cpu=native",
                str(source.with_suffix(".rs")),
                "-o",
                str(rust),
            ]
        )
        commands["rust_ms"] = [str(rust)]
    if available("julia"):
        commands["julia_ms"] = [
            "julia",
            "--startup-file=no",
            str(source.with_suffix(".jl")),
        ]
    try:
        subprocess.run(
            ["python3", "-c", "import numpy"],
            cwd=ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        commands["numpy_ms"] = ["python3", str(source.with_suffix(".py"))]
    except (FileNotFoundError, subprocess.CalledProcessError):
        pass
    if available("bun"):
        commands["js_ms"] = ["bun", str(source.with_suffix(".ts"))]
    return commands


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=7)
    parser.add_argument("--bootstrap", action="store_true")
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        default=ROOT / "benchmarks/observatory.tsv",
    )
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")

    BUILD.mkdir(parents=True, exist_ok=True)
    run(["cargo", "build", "--release", "-p", "lu"])
    if args.bootstrap:
        run([str(ROOT / "selfhost/build.sh"), "--bootstrap"])
    headers = [
        "date",
        "kernel",
        "lulang_aot_ms",
        "lulang_jit_ms",
        "lulang_selfhost_ms",
        "cpp_o3_ms",
        "cpp_fast_ms",
        "rust_ms",
        "julia_ms",
        "numpy_ms",
        "js_ms",
        "lu_source",
        "cpp_source",
        "rust_source",
        "julia_source",
        "numpy_source",
        "js_source",
        "assumptions_layout",
    ]
    rows = []
    for name, label, assumptions in [
        (
            "dot",
            "dot 2M×20",
            "order-free sum; approximate FP; contiguous f64 vectors; whole process",
        ),
        (
            "slerp",
            "slerp 2M",
            "approximate FP; value quaternions; NumPy uses a vectorized batch; whole process",
        ),
    ]:
        measurements: dict[str, str] = {}
        answers = []
        for column, command in compile_kernel(name).items():
            elapsed, answer = measure(command, args.runs)
            measurements[column] = f"{elapsed:.3f}"
            answers.append((column, answer))
        reference = answers[0][1]
        for column, answer in answers[1:]:
            if not math.isclose(answer, reference, rel_tol=1e-8, abs_tol=1e-6):
                raise RuntimeError(
                    f"{name}: {column} produced {answer}, expected approximately {reference}"
                )
        row = {
            "date": dt.datetime.now(dt.timezone.utc).date().isoformat(),
            "kernel": label,
            **measurements,
            "lu_source": f"corpus/bench_{name}.lu",
            "cpp_source": f"corpus/bench_{name}.cpp",
            "rust_source": f"corpus/bench_{name}.rs",
            "julia_source": f"corpus/bench_{name}.jl",
            "numpy_source": f"corpus/bench_{name}.py",
            "js_source": f"corpus/bench_{name}.ts",
            "assumptions_layout": assumptions,
        }
        rows.append(row)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        "\t".join(headers)
        + "\n"
        + "".join(
            "\t".join(row.get(header, "") for header in headers) + "\n"
            for row in rows
        )
    )
    environment = {
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "git_commit": run(["git", "rev-parse", "HEAD"], capture=True),
        "git_dirty": bool(run(["git", "status", "--porcelain"], capture=True)),
        "runs_per_command": args.runs,
        "platform": platform.platform(),
        "machine": platform.machine(),
        "processor": platform.processor(),
        "python": platform.python_version(),
        "tools": {
            "clang": version(["clang++", "--version"]),
            "rust": version(["rustc", "--version"]),
            "julia": version(["julia", "--version"]),
            "numpy": version(
                ["python3", "-c", "import numpy; print(numpy.__version__)"]
            ),
            "bun": version(["bun", "--version"]),
        },
        "environment": {
            key: os.environ[key]
            for key in ["LU_MATH", "LU_IFCONV", "LU_LICM", "LU_SIMD", "LU_LAYOUT"]
            if key in os.environ
        },
    }
    args.output.with_name("environment.json").write_text(
        json.dumps(environment, indent=2) + "\n"
    )
    print(args.output)


if __name__ == "__main__":
    main()

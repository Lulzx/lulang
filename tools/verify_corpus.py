"""Verify the checked corpus across the four compiler tiers.

Full benchmark inputs are compared across JIT, host AOT, and self-hosted AOT.
The reference interpreter also runs a mechanically scaled version of those
inputs so the same source shapes remain in differential coverage without
turning a correctness gate into a multi-minute benchmark.
"""

from __future__ import annotations

import argparse
import math
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
BENCHMARK_REWRITES = {
    "bench_dot.lu": [
        ("let n = 2000000", "let n = 2000"),
        ("for r in 0..20", "for r in 0..2"),
    ],
    "bench_qnorm.lu": [
        ("let n = 2000000", "let n = 2000"),
        ("for r in 0..20", "for r in 0..2"),
    ],
    "bench_slerp.lu": [("let n = 2000000", "let n = 2000")],
}


def run(arguments: list[os.PathLike[str] | str], *, cwd: Path = ROOT) -> str:
    result = subprocess.run(arguments, cwd=cwd, capture_output=True, text=True)
    if result.returncode:
        command = " ".join(map(str, arguments))
        raise RuntimeError(
            f"command failed ({result.returncode}): {command}\n"
            f"{result.stdout}{result.stderr}"
        )
    return result.stdout


def equivalent(expected: str, actual: str) -> bool:
    expected_tokens = expected.split()
    actual_tokens = actual.split()
    if len(expected_tokens) != len(actual_tokens):
        return False
    for expected_token, actual_token in zip(expected_tokens, actual_tokens):
        if expected_token == actual_token:
            continue
        try:
            expected_number = float(expected_token)
            actual_number = float(actual_token)
        except ValueError:
            return False
        if not math.isclose(
            expected_number,
            actual_number,
            rel_tol=2e-13,
            abs_tol=1e-14,
        ):
            return False
    return True


def assert_outputs(source: Path, outputs: dict[str, str]) -> None:
    reference_tier, reference = next(iter(outputs.items()))
    for tier, output in list(outputs.items())[1:]:
        if not equivalent(reference, output):
            raise AssertionError(
                f"{source.name}: {tier} disagrees with {reference_tier}\n"
                f"{reference_tier}: {reference!r}\n{tier}: {output!r}"
            )


def compiled_outputs(lu: Path, source: Path, directory: Path) -> dict[str, str]:
    host = directory / f"{source.stem}.host"
    run([lu, "build", "-o", host, source])
    selfhost = directory / f"{source.stem}.selfhost"
    run([ROOT / "selfhost/build.sh", source, "-o", selfhost])
    return {
        "JIT": run([lu, "run", source]),
        "host AOT": run([host]),
        "selfhost AOT": run([selfhost]),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--lu",
        type=Path,
        default=Path(os.environ.get("LULANG_BIN", ROOT / "target/release/lu")),
    )
    arguments = parser.parse_args()
    lu = arguments.lu.resolve()
    if not lu.is_file():
        raise SystemExit(f"cannot find compiler: {lu}")

    with tempfile.TemporaryDirectory(prefix="lulang-corpus-") as temporary:
        directory = Path(temporary)
        for source in sorted((ROOT / "corpus").glob("*.lu")):
            if source.name in BENCHMARK_REWRITES:
                full_outputs = compiled_outputs(lu, source, directory)
                assert_outputs(source, full_outputs)

                scaled_source = directory / source.name
                scaled = source.read_text()
                for old, new in BENCHMARK_REWRITES[source.name]:
                    if old not in scaled:
                        raise AssertionError(
                            f"{source.name}: scaling anchor disappeared: {old}"
                        )
                    scaled = scaled.replace(old, new)
                scaled_source.write_text(scaled)
                scaled_outputs = {
                    "reference interpreter": run([lu, "interp", scaled_source]),
                    **compiled_outputs(lu, scaled_source, directory),
                }
                assert_outputs(scaled_source, scaled_outputs)
                print(f"{source.name}: full compiled + scaled four-tier agreement")
                continue

            outputs = {
                "reference interpreter": run([lu, "interp", source]),
                **compiled_outputs(lu, source, directory),
            }
            assert_outputs(source, outputs)
            print(f"{source.name}: four-tier agreement")


if __name__ == "__main__":
    main()

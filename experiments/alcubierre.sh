#!/usr/bin/env bash
# alcubierre.sh — replicate AE's "alcubierre" benchmark table with lulang.
# Measures whole-process wall time (hyperfine) in three categories:
#   native binary   : lu build output  vs  clang++ -O3 -march=native
#   JIT / runtime   : lu run (Cranelift) vs bun run (TS)
#   compile time    : lu build          vs clang++ -O3 -march=native
# Usage: experiments/alcubierre.sh   (from the repo root; needs hyperfine, bun)
set -euo pipefail
cd "$(dirname "$0")/.."
LU=./target/release/lu
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

$LU build corpus/alcubierre.lu >/dev/null && mv alcubierre "$OUT/alc-lu"
clang++ -O3 -march=native -o "$OUT/alc-cpp" corpus/alcubierre.cpp

echo "== sanity: outputs must agree =="
"$OUT/alc-lu"; "$OUT/alc-cpp"; $LU run corpus/alcubierre.lu; bun run corpus/alcubierre.ts

echo; echo "== native binary =="
hyperfine --warmup 10 --min-runs 60 -N \
  -n lu-bin "$OUT/alc-lu" -n cpp-bin "$OUT/alc-cpp"

echo "== JIT / runtime =="
hyperfine --warmup 3 --min-runs 25 \
  -n "lu jit" "$LU run corpus/alcubierre.lu" \
  -n "bun run" "bun run corpus/alcubierre.ts" \
  -n "bun run (obj)" "bun run corpus/alcubierre_obj.ts"

echo "== compile time (-O3 -march=native) =="
cp corpus/alcubierre.lu corpus/alcubierre.cpp "$OUT/"
hyperfine --warmup 3 --min-runs 25 \
  -n "lu build" "cd $OUT && $PWD/$LU build alcubierre.lu" \
  -n "clang++" "clang++ -O3 -march=native -o $OUT/alc-cpp2 $OUT/alcubierre.cpp"

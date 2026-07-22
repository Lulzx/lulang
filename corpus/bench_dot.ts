// bench_dot.ts — TS twin of bench_dot.lu (typed arrays: Bun's best case).
function dot(a: Float64Array, b: Float64Array, n: number): number {
  let s = 0.0;
  for (let i = 0; i < n; i++) s += a[i] * b[i];
  return s;
}

const n = 2_000_000;
const a = new Float64Array(n);
const b = new Float64Array(n);
for (let i = 0; i < n; i++) {
  a[i] = i * 0.000001;
  b[i] = i * 0.000002;
}
let acc = 0.0;
for (let r = 0; r < 20; r++) acc += dot(a, b, n);
console.log("acc:", acc);

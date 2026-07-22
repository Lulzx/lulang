// bench_qnorm.ts — TS twin of bench_qnorm.lu (typed arrays, SoA by hand:
// Bun's best case; idiomatic object arrays would be far slower).
const n = 2_000_000;
const w = new Float64Array(n);
const x = new Float64Array(n);
const y = new Float64Array(n);
const z = new Float64Array(n);
for (let i = 0; i < n; i++) {
  const f = i * 0.000001;
  w[i] = 1.0 + f;
  x[i] = 2.0 - f;
  y[i] = f * 3.0;
  z[i] = 0.5;
}

function qq(): number {
  let s = 0.0;
  for (let i = 0; i < n; i++) s += w[i] * w[i] + x[i] * x[i] + y[i] * y[i] + z[i] * z[i];
  return s;
}

let acc = 0.0;
for (let r = 0; r < 20; r++) acc += qq();
console.log("acc:", acc);

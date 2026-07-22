// Same kernels, idiomatic TypeScript, run under Bun — the "5.5-9x slower than AE jit" baseline.
// Two styles per kernel where it matters: idiomatic objects vs typed arrays.

function nowMs(): number { return performance.now(); }

let rngState = 88172645463325252n;
const MASK = (1n << 64n) - 1n;
function frand(): number {
  rngState = (rngState ^ (rngState << 13n)) & MASK;
  rngState = rngState ^ (rngState >> 7n);
  rngState = (rngState ^ (rngState << 17n)) & MASK;
  return Number(rngState >> 11n) / 9007199254740992.0;
}

// ---------- dot ----------
function kernelDot(a: Float64Array, b: Float64Array): number {
  let s = 0.0;
  for (let i = 0; i < a.length; i++) s += a[i] * b[i];
  return s;
}

// ---------- nbody (typed arrays, best-case TS) ----------
function kernelNbody(x: Float64Array, y: Float64Array, z: Float64Array,
                     ax: Float64Array, ay: Float64Array, az: Float64Array, n: number): number {
  for (let i = 0; i < n; i++) { ax[i] = 0; ay[i] = 0; az[i] = 0; }
  for (let i = 0; i < n; i++) {
    let axi = 0, ayi = 0, azi = 0;
    const xi = x[i], yi = y[i], zi = z[i];
    for (let j = 0; j < n; j++) {
      const dx = x[j] - xi, dy = y[j] - yi, dz = z[j] - zi;
      const r2 = dx * dx + dy * dy + dz * dz + 1e-9;
      const inv = 1.0 / Math.sqrt(r2);
      const inv3 = inv * inv * inv;
      axi += dx * inv3; ayi += dy * inv3; azi += dz * inv3;
    }
    ax[i] = axi; ay[i] = ayi; az[i] = azi;
  }
  let s = 0;
  for (let i = 0; i < n; i++) s += ax[i] + ay[i] + az[i];
  return s;
}

// ---------- slerp, idiomatic objects (how people actually write TS) ----------
interface Quat { w: number; x: number; y: number; z: number; }

function slerp1(a: Quat, b: Quat, t: number): Quat {
  let d = a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z;
  let bw = b.w, bx = b.x, by = b.y, bz = b.z;
  if (d < 0) { d = -d; bw = -bw; bx = -bx; by = -by; bz = -bz; }
  let wa: number, wb: number;
  if (d > 0.9995) { wa = 1 - t; wb = t; }
  else {
    const th = Math.acos(d), s = Math.sin(th);
    wa = Math.sin((1 - t) * th) / s; wb = Math.sin(t * th) / s;
  }
  const rw = wa * a.w + wb * bw, rx = wa * a.x + wb * bx;
  const ry = wa * a.y + wb * by, rz = wa * a.z + wb * bz;
  const n = 1 / Math.sqrt(rw * rw + rx * rx + ry * ry + rz * rz);
  return { w: rw * n, x: rx * n, y: ry * n, z: rz * n };
}

function kernelSlerpObjects(A: Quat[], B: Quat[]): number {
  let s = 0;
  for (let i = 0; i < A.length; i++) {
    const q = slerp1(A[i], B[i], 0.37);
    s += q.w + q.x + q.y + q.z;
  }
  return s;
}

function bestMs(f: () => number, reps: number): number {
  let best = Infinity;
  for (let r = 0; r < reps; r++) {
    const t0 = nowMs();
    f();
    const dt = nowMs() - t0;
    if (dt < best) best = dt;
  }
  return best;
}

const ND = 2_000_000, NB = 1500, NQ = 200_000;

const da = new Float64Array(ND), db = new Float64Array(ND);
for (let i = 0; i < ND; i++) { da[i] = frand() - 0.5; db[i] = frand() - 0.5; }

const bx = new Float64Array(NB), by = new Float64Array(NB), bz = new Float64Array(NB);
const bax = new Float64Array(NB), bay = new Float64Array(NB), baz = new Float64Array(NB);
for (let i = 0; i < NB; i++) { bx[i] = frand(); by[i] = frand(); bz[i] = frand(); }

const qa: Quat[] = [], qb: Quat[] = [];
for (let i = 0; i < NQ; i++) {
  const q1 = { w: frand() - .5, x: frand() - .5, y: frand() - .5, z: frand() - .5 };
  const q2 = { w: frand() - .5, x: frand() - .5, y: frand() - .5, z: frand() - .5 };
  const n1 = 1 / Math.sqrt(q1.w ** 2 + q1.x ** 2 + q1.y ** 2 + q1.z ** 2);
  const n2 = 1 / Math.sqrt(q2.w ** 2 + q2.x ** 2 + q2.y ** 2 + q2.z ** 2);
  qa.push({ w: q1.w * n1, x: q1.x * n1, y: q1.y * n1, z: q1.z * n1 });
  qb.push({ w: q2.w * n2, x: q2.x * n2, y: q2.y * n2, z: q2.z * n2 });
}

const R = 30;
console.log(`dot,${bestMs(() => kernelDot(da, db), R).toFixed(3)}`);
console.log(`nbody,${bestMs(() => kernelNbody(bx, by, bz, bax, bay, baz, NB), R).toFixed(3)}`);
console.log(`slerp_obj,${bestMs(() => kernelSlerpObjects(qa, qb), R).toFixed(3)}`);

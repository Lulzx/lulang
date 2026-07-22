// bench_slerp.ts — TS twin of bench_slerp.lu, idiomatic object style.
interface Quat { w: number; x: number; y: number; z: number; }

const dotq = (a: Quat, b: Quat) => a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z;
const norm = (q: Quat) => Math.sqrt(dotq(q, q));
const scale = (q: Quat, s: number): Quat => ({ w: q.w * s, x: q.x * s, y: q.y * s, z: q.z * s });
const addq = (a: Quat, b: Quat): Quat => ({ w: a.w + b.w, x: a.x + b.x, y: a.y + b.y, z: a.z + b.z });
const normalize = (q: Quat) => scale(q, 1 / norm(q));

function slerp(a: Quat, b: Quat, t: number): Quat {
  let d = dotq(a, b);
  let bb = b;
  if (d < 0) { d = -d; bb = scale(b, -1); }
  let wa = 1 - t, wb = t;
  if (d < 0.9995) {
    const th = Math.acos(d), s = Math.sin(th);
    wa = Math.sin((1 - t) * th) / s;
    wb = Math.sin(t * th) / s;
  }
  return normalize(addq(scale(a, wa), scale(bb, wb)));
}

const a = normalize({ w: 1, x: 2, y: 3, z: 4 });
const b = normalize({ w: 4, x: 3, y: 2, z: 1 });
const n = 2_000_000;
let acc = 0.0;
for (let i = 0; i < n; i++) {
  const t = i / n;
  acc += norm(slerp(a, b, t));
}
console.log("acc:", acc);

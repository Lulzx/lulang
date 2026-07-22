// alcubierre_obj.ts — same math, idiomatic object style: one {x,y,z} record
// per grid point, as a typical TS codebase would write it. Tests whether AE's
// ~9x JIT-vs-bun gap comes from allocating-object baselines.
interface P { x: number; y: number; z: number; }

function rho(p: P, xs: number): number {
  const rx = p.x - xs;
  const r2 = rx * rx + p.y * p.y + p.z * p.z;
  const rs = Math.sqrt(r2);
  const u = r2 / 4.0;
  const w = 1.0 + u * u * u;
  const dfdr = (-6.0 * u * u * rs) / 4.0 / (w * w);
  return (0.25 * dfdr * dfdr * (p.y * p.y + p.z * p.z)) / r2;
}

const n = 96;
const dx = 16.0 / n;
let total = 0.0;
for (let it = 0; it < 6; it++) {
  const xs = -4.0 + it * 0.5;
  for (let ix = 0; ix < n; ix++) {
    const x = -8.0 + (ix + 0.5) * dx;
    for (let iy = 0; iy < n; iy++) {
      const y = -8.0 + (iy + 0.5) * dx;
      let acc = 0.0;
      for (let iz = 0; iz < n; iz++) {
        const p: P = { x, y, z: -8.0 + (iz + 0.5) * dx };
        acc += rho(p, xs);
      }
      total += acc;
    }
  }
}
console.log("total:", total * dx * dx * dx);

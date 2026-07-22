// TS twin of alcubierre.lu — idiomatic, run with bun.
function rho(x: number, y: number, z: number, xs: number): number {
  const rx = x - xs;
  const r2 = rx * rx + y * y + z * z;
  const rs = Math.sqrt(r2);
  const u = r2 / 4.0;
  const w = 1.0 + u * u * u;
  const dfdr = (-6.0 * u * u * rs) / 4.0 / (w * w);
  return (0.25 * dfdr * dfdr * (y * y + z * z)) / r2;
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
        const z = -8.0 + (iz + 0.5) * dx;
        acc += rho(x, y, z, xs);
      }
      total += acc;
    }
  }
}
console.log("total:", total * dx * dx * dx);

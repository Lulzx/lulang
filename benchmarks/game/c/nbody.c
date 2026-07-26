/* n-body — same algorithm and SoA layout as nbody.lu, single-threaded. */
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

#define N 5

static double px[N], py[N], pz[N], vx[N], vy[N], vz[N], mass[N];

static double energy(void) {
  double e = 0.0;
  for (int i = 0; i < N; i++) {
    e += 0.5 * mass[i] * (vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]);
    for (int j = i + 1; j < N; j++) {
      double dx = px[i] - px[j], dy = py[i] - py[j], dz = pz[i] - pz[j];
      e -= mass[i] * mass[j] / sqrt(dx * dx + dy * dy + dz * dz);
    }
  }
  return e;
}

int main(int argc, char **argv) {
  int steps = argc > 1 ? atoi(argv[1]) : 1000;
  const double pi = 3.141592653589793;
  const double solar_mass = 4.0 * pi * pi;
  const double dpy = 365.24;

  mass[0] = solar_mass;

  px[1] = 4.84143144246472090;
  py[1] = -1.16032004402742839;
  pz[1] = -0.103622044471123109;
  vx[1] = 0.00166007664274403694 * dpy;
  vy[1] = 0.00769901118419740425 * dpy;
  vz[1] = -0.0000690460016972063023 * dpy;
  mass[1] = 0.000954791938424326609 * solar_mass;

  px[2] = 8.34336671824457987;
  py[2] = 4.12479856412430479;
  pz[2] = -0.403523417114321381;
  vx[2] = -0.00276742510726862411 * dpy;
  vy[2] = 0.00499852801234917238 * dpy;
  vz[2] = 0.0000230417297573763929 * dpy;
  mass[2] = 0.000285885980666130812 * solar_mass;

  px[3] = 12.8943695621391310;
  py[3] = -15.1111514016986312;
  pz[3] = -0.223307578892655734;
  vx[3] = 0.00296460137564761618 * dpy;
  vy[3] = 0.00237847173959480950 * dpy;
  vz[3] = -0.0000296589568540237556 * dpy;
  mass[3] = 0.0000436624404335156298 * solar_mass;

  px[4] = 15.3796971148509165;
  py[4] = -25.9193146099879641;
  pz[4] = 0.179258772950371181;
  vx[4] = 0.00268067772490389322 * dpy;
  vy[4] = 0.00162824170038242295 * dpy;
  vz[4] = -0.0000951592254519715870 * dpy;
  mass[4] = 0.0000515138902046611451 * solar_mass;

  double ox = 0.0, oy = 0.0, oz = 0.0;
  for (int i = 0; i < N; i++) {
    ox += vx[i] * mass[i];
    oy += vy[i] * mass[i];
    oz += vz[i] * mass[i];
  }
  vx[0] = -ox / solar_mass;
  vy[0] = -oy / solar_mass;
  vz[0] = -oz / solar_mass;

  printf("%.9f\n", energy());

  const double dt = 0.01;
  for (int s = 0; s < steps; s++) {
    for (int i = 0; i < N; i++) {
      for (int j = i + 1; j < N; j++) {
        double dx = px[i] - px[j], dy = py[i] - py[j], dz = pz[i] - pz[j];
        double d2 = dx * dx + dy * dy + dz * dz;
        double mag = dt / (d2 * sqrt(d2));
        double mi = mass[i] * mag, mj = mass[j] * mag;
        vx[i] -= dx * mj; vy[i] -= dy * mj; vz[i] -= dz * mj;
        vx[j] += dx * mi; vy[j] += dy * mi; vz[j] += dz * mi;
      }
    }
    for (int i = 0; i < N; i++) {
      px[i] += dt * vx[i];
      py[i] += dt * vy[i];
      pz[i] += dt * vz[i];
    }
  }

  printf("%.9f\n", energy());
  return 0;
}

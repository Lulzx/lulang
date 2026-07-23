// Rust twin of bench_slerp.lu — an idiomatic value-type quaternion.

#[derive(Clone, Copy)]
struct Quat {
    w: f64,
    x: f64,
    y: f64,
    z: f64,
}

fn dot(a: Quat, b: Quat) -> f64 {
    a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z
}

fn norm(q: Quat) -> f64 {
    dot(q, q).sqrt()
}

fn scale(q: Quat, s: f64) -> Quat {
    Quat {
        w: q.w * s,
        x: q.x * s,
        y: q.y * s,
        z: q.z * s,
    }
}

fn add(a: Quat, b: Quat) -> Quat {
    Quat {
        w: a.w + b.w,
        x: a.x + b.x,
        y: a.y + b.y,
        z: a.z + b.z,
    }
}

fn normalize(q: Quat) -> Quat {
    scale(q, 1.0 / norm(q))
}

fn slerp(a: Quat, b: Quat, t: f64) -> Quat {
    let mut d = dot(a, b);
    let mut bb = b;
    if d < 0.0 {
        d = -d;
        bb = scale(b, -1.0);
    }
    let (mut wa, mut wb) = (1.0 - t, t);
    if d < 0.9995 {
        let theta = d.acos();
        let sine = theta.sin();
        wa = ((1.0 - t) * theta).sin() / sine;
        wb = (t * theta).sin() / sine;
    }
    normalize(add(scale(a, wa), scale(bb, wb)))
}

fn main() {
    let a = normalize(Quat {
        w: 1.0,
        x: 2.0,
        y: 3.0,
        z: 4.0,
    });
    let b = normalize(Quat {
        w: 4.0,
        x: 3.0,
        y: 2.0,
        z: 1.0,
    });
    const N: usize = 2_000_000;
    let mut acc = 0.0;
    for i in 0..N {
        acc += norm(slerp(a, b, i as f64 / N as f64));
    }
    println!("acc: {acc:.17}");
}

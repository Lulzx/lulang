// Rust twin of bench_dot.lu — idiomatic owned vectors and a scalar iterator.

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn main() {
    const N: usize = 2_000_000;
    let a = (0..N).map(|i| i as f64 * 0.000001).collect::<Vec<_>>();
    let b = (0..N).map(|i| i as f64 * 0.000002).collect::<Vec<_>>();
    let mut acc = 0.0;
    for _ in 0..20 {
        acc += dot(&a, &b);
    }
    println!("acc: {acc:.17}");
}

//! The reference interpreter must keep element assignment O(1).
//!
//! `a[i] = x` lowers to a `Load` of the array plus a `SetIndex`. If the loaded
//! alias is still live in the SSA value table when the update runs, the
//! interpreter's copy-on-write deep-copies the whole array on every store and
//! array loops become quadratic — `corpus/dot.lu` took 105 s before the
//! liveness pass in `src/interp.rs` released dead slots. Nothing else in the
//! suite notices, because the interpreted corpus inputs are all small.

use lu_ir::ir::LoweredProgram;
use lu_syntax::{lexer, parser};
use lu_test::interp::Interp;
use std::time::{Duration, Instant};

fn fill_and_sum(n: usize) -> String {
    format!(
        "main {{\n  \
           let n = {n}\n  \
           var a = arr(n, 0.0)\n  \
           for i in 0..n {{\n    a[i] = float(i) * 0.5\n  }}\n  \
           var total = 0.0\n  \
           for i in 0..n {{\n    total = total + a[i]\n  }}\n  \
           print(total)\n\
         }}\n"
    )
}

fn interpret(source: &str) -> Duration {
    let tokens = lexer::lex(source).expect("lex");
    let mut parser = parser::Parser::new(tokens);
    parser.parse().expect("parse");
    let ir = LoweredProgram::lower(parser.prog).expect("lower");
    let interp = Interp::new(&ir);

    let mut best = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        interp.run_main().expect("run");
        best = best.min(start.elapsed());
    }
    best
}

#[test]
fn array_stores_do_not_scale_quadratically() {
    let small = interpret(&fill_and_sum(8_000));
    let large = interpret(&fill_and_sum(32_000));

    // 4× the work. Linear lands near 4×; the quadratic regression lands near
    // 16×. The 8× gate leaves room for timer noise and allocator warmup while
    // still failing decisively if per-store copying comes back.
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 8.0,
        "interpreting 4× the array stores took {ratio:.1}× longer \
         ({small:?} → {large:?}); element assignment is copying the array"
    );
}

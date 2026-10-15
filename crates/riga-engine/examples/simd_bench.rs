//! Worst-of kernel microbenchmark: vectorised vs scalar (Phase 8 evidence).
//!
//! Run with `cargo run --release -p riga-engine --example simd_bench`.

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use riga_engine::simd::{worst_of_two, worst_of_two_scalar};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let n = 1_000_000usize;
    let reps = 50usize;
    let mut rng = ChaCha8Rng::seed_from_u64(2026);
    let a: Vec<f32> = (0..n).map(|_| rng.random_range(0.2..2.0)).collect();
    let b: Vec<f32> = (0..n).map(|_| rng.random_range(0.2..2.0)).collect();
    let mut out = vec![0.0f32; n];

    let t0 = Instant::now();
    for _ in 0..reps {
        worst_of_two_scalar(black_box(&a), black_box(&b), black_box(&mut out));
    }
    let scalar = t0.elapsed();

    let t1 = Instant::now();
    for _ in 0..reps {
        worst_of_two(black_box(&a), black_box(&b), black_box(&mut out));
    }
    let vector = t1.elapsed();

    let ratio = scalar.as_secs_f64() / vector.as_secs_f64();
    println!(
        "worst-of n={n} reps={reps}: scalar={:?} vector={:?} speedup={ratio:.2}x",
        scalar, vector
    );
}

//! GPU worst-of throughput vs CPU at a 4096-path block (Phase 9 evidence).
//!
//! Run with `cargo run --release -p riga-engine --features gpu --example gpu_bench`.
//! Without the `gpu` feature the example prints how to build it.

fn main() {
    #[cfg(not(feature = "gpu"))]
    {
        println!("GPU backend disabled: rebuild with `--features gpu`");
    }
    #[cfg(feature = "gpu")]
    {
        run();
    }
}

#[cfg(feature = "gpu")]
fn run() {
    use riga_engine::simd::worst_of_two;
    use std::hint::black_box;
    use std::time::Instant;

    println!(
        "device: {} (target {})",
        riga_engine::gpu::device_name().unwrap(),
        riga_engine::gpu::arch().unwrap()
    );
    println!("worst-of reduction, 1260 steps x 2 factors per path\n");
    println!(
        "{:>8}  {:>10}  {:>10}  {:>10}  {:>8}",
        "paths", "CPU ms", "GPU ms", "GPU resv", "speedup"
    );

    let times = 1260usize;
    for &paths in &[4096usize, 16384, 65536] {
        let n = paths * times;
        let mut a = vec![0.0f32; n];
        let mut b = vec![0.0f32; n];
        let mut seed = 0x9e37_79b9u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 + 0.1
        };
        for i in 0..n {
            a[i] = next();
            b[i] = next();
        }
        let reps = 20usize;

        let mut out = vec![0.0f32; n];
        let t0 = Instant::now();
        for _ in 0..reps {
            worst_of_two(black_box(&a), black_box(&b), black_box(&mut out));
        }
        let cpu = t0.elapsed().as_secs_f64() / reps as f64;

        // Per-call: uploads both inputs and downloads the result every call.
        let t1 = Instant::now();
        for _ in 0..reps {
            black_box(riga_engine::gpu::worst_of(black_box(&a), black_box(&b)).unwrap());
        }
        let gpu_call = t1.elapsed().as_secs_f64() / reps as f64;

        // Resident: upload once, run the kernel many times, download once.
        let mut resident = riga_engine::gpu::resident_worst_of(&a, &b).unwrap();
        resident.synchronize().unwrap();
        let t2 = Instant::now();
        for _ in 0..reps {
            resident.reduce().unwrap();
        }
        resident.synchronize().unwrap();
        let gpu_resident = t2.elapsed().as_secs_f64() / reps as f64;
        black_box(resident.download().unwrap());

        println!(
            "{paths:>8}  {:>10.3}  {:>10.3}  {:>10.3}  {:>8.2}x",
            cpu * 1e3,
            gpu_call * 1e3,
            gpu_resident * 1e3,
            cpu / gpu_resident
        );
    }
    println!(
        "\ncolumns: CPU SIMD ms | GPU per-call ms (H2D+D2H each call) | GPU resident ms (amortised)\n\
         speedup = CPU / GPU-resident. A single 4096 block is transfer-bound; the resident\n\
         kernel wins as blocks aggregate — the documented 'GPU wins at small path counts' non-claim."
    );
}

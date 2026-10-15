//! CPU vs GPU Dupire-LV path propagation on the termsheet1 fixture (Phase 9).
//!
//! Sweeps path counts to find the crossover: a single 30k trade is H2D/D2H transfer-bound,
//! the GPU wins as the resident block grows. Reports the KO-gated production PUT leg.
//!
//! Run: `cargo run --release -p riga-engine --features gpu --example gpu_lv_bench`

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
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;
    use rand_distr::StandardNormal;
    use riga_engine::build_local_vol;
    use riga_engine::fixture::load_termsheet1_fixture;
    use std::path::PathBuf;
    use std::time::Instant;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
    let req = load_termsheet1_fixture(path).unwrap().request;
    let lv = build_local_vol(&req).unwrap().unwrap();
    let steps = req.trade.steps;
    let dt = req.trade.maturity_years / steps as f64;
    let rate = req.market.risk_free_rate;
    let strike = req.trade.strike;
    let ki = req.trade.knock_in_barrier;
    let call_barrier = req.trade.knock_out_barrier.unwrap_or(f64::INFINITY);
    let init = [req.trade.initial_spots[0], req.trade.initial_spots[1]];
    let chol = riga_market::CorrelationMatrix::from_rows(&req.market.correlation)
        .unwrap()
        .cholesky()
        .unwrap();

    // Paths-independent per-step local-vol rows [factor, step, gx] + per-factor axes.
    let gx = lv[0].log_strike_axis().len();
    let mut rows = vec![0.0f32; 2 * steps * gx];
    let mut ls_axis = Vec::with_capacity(2 * gx);
    for f in 0..2 {
        for s in 0..steps {
            let row = lv[f].row(s as f64 * dt);
            for j in 0..gx {
                rows[(f * steps + s) * gx + j] = row[j] as f32;
            }
        }
        ls_axis.extend(lv[f].log_strike_axis().iter().map(|&x| x as f32));
    }
    let s0 = [req.market.spots[0] as f32, req.market.spots[1] as f32];
    let refs = [init[0] as f32, init[1] as f32];
    let divs = [
        req.market.dividend_yields[0] as f32,
        req.market.dividend_yields[1] as f32,
    ];
    let df = (-rate * req.trade.maturity_years).exp();

    println!("device: {}", riga_engine::gpu::device_name().unwrap());
    println!(
        "{:>8} {:>10} {:>10} {:>9}  {:>9} {:>9} {:>9}",
        "paths", "CPU s", "GPU s", "speedup", "CPU gated", "GPU gated", "gated rel"
    );

    for paths in [30_000usize, 100_000, 200_000] {
        // CRN shocks generated once; excluded from both timings.
        let mut rng = ChaCha8Rng::seed_from_u64(req.config.seed);
        let mut shocks = vec![0.0f32; paths * steps * 2];
        for p in 0..paths {
            for s in 0..steps {
                let z0: f64 = rng.sample(StandardNormal);
                let z1: f64 = rng.sample(StandardNormal);
                let mut sh = [0.0f64; 2];
                chol.apply_into(&[z0, z1], &mut sh);
                shocks[(p * steps + s) * 2] = sh[0] as f32;
                shocks[(p * steps + s) * 2 + 1] = sh[1] as f32;
            }
        }

        // CPU reference (f64), KO-gated.
        let t_cpu = Instant::now();
        let mut cpu_gated = 0.0f64;
        for p in 0..paths {
            let mut spots = [req.market.spots[0], req.market.spots[1]];
            let mut ko = false;
            for s in 0..steps {
                for f in 0..2 {
                    let vol = lv[f].sigma(s as f64 * dt, spots[f]);
                    let drift = (rate - req.market.dividend_yields[f] - 0.5 * vol * vol) * dt;
                    spots[f] *=
                        (drift + vol * dt.sqrt() * shocks[(p * steps + s) * 2 + f] as f64).exp();
                }
                if !ko && spots[0] / init[0] >= call_barrier && spots[1] / init[1] >= call_barrier {
                    ko = true;
                }
            }
            let worst = (spots[0] / init[0]).min(spots[1] / init[1]);
            if worst <= ki && !ko {
                cpu_gated += (strike - worst).max(0.0);
            }
        }
        let cpu_gated = cpu_gated / paths as f64 * df;
        let cpu_s = t_cpu.elapsed().as_secs_f64();

        // GPU (f32), single launch, KO step returned.
        let t_gpu = Instant::now();
        let (term, ko) = riga_engine::gpu::lv_terminal_spots(
            &shocks,
            &rows,
            &ls_axis,
            &s0,
            &refs,
            call_barrier as f32,
            paths,
            steps,
            dt as f32,
            rate as f32,
            &divs,
        )
        .unwrap();
        let mut gpu_gated = 0.0f64;
        for p in 0..paths {
            let worst = ((term[p * 2] as f64) / init[0]).min((term[p * 2 + 1] as f64) / init[1]);
            if worst <= ki && ko[p] < 0 {
                gpu_gated += (strike - worst).max(0.0);
            }
        }
        let gpu_gated = gpu_gated / paths as f64 * df;
        let gpu_s = t_gpu.elapsed().as_secs_f64();

        println!(
            "{paths:>8} {cpu_s:>10.3} {gpu_s:>10.3} {:>8.2}x  {cpu_gated:>9.6} {gpu_gated:>9.6} {:>9.2e}",
            cpu_s / gpu_s,
            (cpu_gated - gpu_gated).abs() / cpu_gated
        );
    }
    println!("(CPU f64 vs GPU f32 including H2D/D2H; gated PUT anchor 0.020329)");
}

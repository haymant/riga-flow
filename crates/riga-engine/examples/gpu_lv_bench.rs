//! CPU vs GPU Dupire-LV path propagation on the termsheet1 fixture (Phase 9).
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
    let paths = req.config.paths;
    let steps = req.trade.steps;
    let dt = req.trade.maturity_years / steps as f64;
    let rate = req.market.risk_free_rate;
    let chol = riga_market::CorrelationMatrix::from_rows(&req.market.correlation)
        .unwrap()
        .cholesky()
        .unwrap();

    // Correlated shocks [path, step, 2] (CRN): generated once, shared by CPU and GPU.
    let mut rng = ChaCha8Rng::seed_from_u64(req.config.seed);
    let mut shocks_f32 = vec![0.0f32; paths * steps * 2];
    for p in 0..paths {
        for s in 0..steps {
            let z0: f64 = rng.sample(StandardNormal);
            let z1: f64 = rng.sample(StandardNormal);
            let mut sh = [0.0f64; 2];
            chol.apply_into(&[z0, z1], &mut sh);
            shocks_f32[(p * steps + s) * 2] = sh[0] as f32;
            shocks_f32[(p * steps + s) * 2 + 1] = sh[1] as f32;
        }
    }

    // Per-factor log-strike axes and per-step local-vol rows [factor, step, gx].
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
    let divs = [
        req.market.dividend_yields[0] as f32,
        req.market.dividend_yields[1] as f32,
    ];
    let df = (-rate * req.trade.maturity_years).exp();

    // CPU reference (f64) — the production `sigma(t, S)` local-vol lookup.
    let t_cpu = Instant::now();
    let mut cpu_sum = 0.0f64;
    for p in 0..paths {
        let mut spots = [req.market.spots[0], req.market.spots[1]];
        for s in 0..steps {
            for f in 0..2 {
                let vol = lv[f].sigma(s as f64 * dt, spots[f]);
                let drift = (rate - req.market.dividend_yields[f] - 0.5 * vol * vol) * dt;
                spots[f] *=
                    (drift + vol * dt.sqrt() * shocks_f32[(p * steps + s) * 2 + f] as f64).exp();
            }
        }
        let worst =
            (spots[0] / req.trade.initial_spots[0]).min(spots[1] / req.trade.initial_spots[1]);
        if worst <= req.trade.knock_in_barrier {
            cpu_sum += (req.trade.strike - worst).max(0.0);
        }
    }
    let cpu_put = cpu_sum / paths as f64 * df;
    let cpu_s = t_cpu.elapsed().as_secs_f64();

    // GPU propagation (f32, one kernel launch per block).
    let t_gpu = Instant::now();
    let term = riga_engine::gpu::lv_terminal_spots(
        &shocks_f32,
        &rows,
        &ls_axis,
        &s0,
        paths,
        steps,
        dt as f32,
        rate as f32,
        &divs,
    )
    .unwrap();
    let mut gpu_sum = 0.0f64;
    for p in 0..paths {
        let worst = ((term[p * 2] as f64) / req.trade.initial_spots[0])
            .min((term[p * 2 + 1] as f64) / req.trade.initial_spots[1]);
        if worst <= req.trade.knock_in_barrier {
            gpu_sum += (req.trade.strike - worst).max(0.0);
        }
    }
    let gpu_put = gpu_sum / paths as f64 * df;
    let gpu_s = t_gpu.elapsed().as_secs_f64();

    println!("device: {}", riga_engine::gpu::device_name().unwrap());
    println!("paths={paths} steps={steps} gx={gx} (per-factor log-strike axes)");
    println!(
        "LV PUT  CPU(f64) {cpu_put:.6} in {cpu_s:.3}s  |  GPU(f32, H2D+kernel+D2H) {gpu_put:.6} in {gpu_s:.3}s"
    );
    println!(
        "|delta| {:.3e}  relative {:.3e}  | propagation speedup {:.2}x",
        (cpu_put - gpu_put).abs(),
        (cpu_put - gpu_put).abs() / cpu_put,
        cpu_s / gpu_s
    );
}

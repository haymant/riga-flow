use riga_engine::{PriceRequest, fixture::load_termsheet1_fixture, price_put_leg};
use serde_json::json;
use std::{
    env,
    io::{self, Read},
    process::ExitCode,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"code":"request_failed","message":error.to_string()})
            );
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("health") {
        println!(
            "{}",
            json!({"version": env!("CARGO_PKG_VERSION"), "engine":"riga-engine", "features":{"cpu_gbm":true,"gpu":riga_engine::gpu_enabled(),"aad":false}})
        );
        return Ok(());
    }
    let request: PriceRequest;
    let expected = if args.get(1).map(String::as_str) == Some("price-put") {
        let path = args
            .get(2)
            .map(String::as_str)
            .unwrap_or("docs/fixture/termsheet1.json");
        let fixture = load_termsheet1_fixture(path)?;
        request = fixture.request.clone();
        Some((
            fixture.expected,
            fixture.source_model,
            fixture.engine_model,
            fixture.market_provenance,
            fixture.market_quality_note,
            fixture.market_is_pinned,
        ))
    } else if args.get(1).map(String::as_str) == Some("price-put-json") {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes)?;
        request = serde_json::from_slice(&bytes)?;
        None
    } else {
        eprintln!(
            "usage: riga health | riga price-put [fixture.json] | riga price-put-json < request.json"
        );
        return Err("unknown command".into());
    };
    let result = price_put_leg(&request)?;
    if let Some((
        expected,
        source_model,
        engine_model,
        market_provenance,
        market_quality_note,
        market_is_pinned,
    )) = expected
    {
        let relative_error =
            (result.legs.down_and_in_put - expected.put_pv).abs() / expected.put_pv;
        let ki_abs_error = (result.ki_probability - expected.ki_probability).abs();
        let numeric_band_check = market_is_pinned.then_some({
            relative_error <= expected.relative_tolerance
                && ki_abs_error <= expected.ki_probability_tolerance
        });
        let status = match numeric_band_check {
            None => "blocked_pending_market_capture",
            Some(true) => "passed",
            Some(false) => "failed",
        };
        println!(
            "{}",
            json!({"result":result,"fixture":{"source_model":source_model,"engine_model":engine_model,"market_provenance":market_provenance,"market_quality_note":market_quality_note},"fixture_acceptance":{"status":status,"passed":numeric_band_check,"numeric_band_check":numeric_band_check,"diagnostic_only":numeric_band_check.is_none(),"put_target":expected.put_pv,"relative_tolerance":expected.relative_tolerance,"diagnostic_relative_error":relative_error,"ki_probability_target":expected.ki_probability,"ki_probability_tolerance":expected.ki_probability_tolerance,"diagnostic_ki_probability_error":ki_abs_error}})
        );
        if numeric_band_check == Some(false) {
            return Err("fixture acceptance failed".into());
        }
    } else {
        println!("{}", serde_json::to_string(&result)?);
    }
    Ok(())
}

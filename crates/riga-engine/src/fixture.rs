use crate::{FixtureExpected, LocalVolSpec, PriceRequest};
use riga_core::{MarketSnapshot, PhoenixTrade, RigaError, SimulationConfig, TraceLevel};
use riga_market::ImpliedVolGrid;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LoadedFixture {
    pub name: String,
    pub request: PriceRequest,
    pub expected: FixtureExpected,
    pub market_provenance: String,
    pub market_is_pinned: bool,
    pub market_quality_note: String,
    pub source_model: String,
    pub engine_model: String,
}

fn number(value: &Value, path: &str) -> Result<f64, RigaError> {
    value.pointer(path).and_then(Value::as_f64).ok_or_else(|| {
        RigaError::new(
            "fixture_missing_field",
            format!("missing numeric fixture field {path}"),
        )
    })
}

fn tenor_years(tenor: &str) -> Option<f64> {
    if let Some(months) = tenor.strip_suffix('M') {
        months.parse::<f64>().ok().map(|months| months / 12.0)
    } else if let Some(years) = tenor.strip_suffix('Y') {
        years.parse::<f64>().ok()
    } else {
        None
    }
}

fn civil_day_number(date: &str) -> Result<i64, RigaError> {
    let mut fields = date.split('-');
    let year = fields.next().and_then(|x| x.parse::<i64>().ok());
    let month = fields.next().and_then(|x| x.parse::<i64>().ok());
    let day = fields.next().and_then(|x| x.parse::<i64>().ok());
    if fields.next().is_some() {
        return Err(RigaError::new(
            "fixture_invalid_date",
            format!("invalid ISO date {date}"),
        ));
    }
    let (Some(mut year), Some(month), Some(day)) = (year, month, day) else {
        return Err(RigaError::new(
            "fixture_invalid_date",
            format!("invalid ISO date {date}"),
        ));
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(RigaError::new(
            "fixture_invalid_date",
            format!("invalid ISO date {date}"),
        ));
    }
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Ok(era * 146_097 + day_of_era)
}

fn weekday_steps(start: &str, end: &str) -> Result<usize, RigaError> {
    let start_day = civil_day_number(start)?;
    let end_day = civil_day_number(end)?;
    if end_day < start_day {
        return Err(RigaError::new(
            "fixture_invalid_date",
            "end date must not precede start date",
        ));
    }
    Ok((start_day + 1..=end_day)
        .filter(|day| (day + 3).rem_euclid(7) < 5)
        .count())
}

fn remaining_business_steps(as_of: &str, maturity: &str) -> Result<(f64, usize), RigaError> {
    let start = civil_day_number(as_of)?;
    let end = civil_day_number(maturity)?;
    if end <= start {
        return Err(RigaError::new(
            "fixture_invalid_date",
            "maturity must be after as_of",
        ));
    }
    let days = end - start;
    // Weekday count is a proxy; NYSE holiday adjustments are not yet applied.
    let weekdays = weekday_steps(as_of, maturity)?;
    Ok((days as f64 / 365.0, weekdays))
}

fn interpolate_curve(points: &[(f64, f64)], time: f64) -> Result<f64, RigaError> {
    if points.is_empty() {
        return Err(RigaError::new(
            "fixture_missing_field",
            "market curve has no points",
        ));
    }
    if !points.windows(2).all(|w| w[0].0 < w[1].0) {
        return Err(RigaError::new(
            "fixture_invalid_field",
            "market curve tenors must be strictly increasing",
        ));
    }
    if time <= points[0].0 {
        return Ok(points[0].1);
    }
    for pair in points.windows(2) {
        if time <= pair[1].0 {
            let weight = (time - pair[0].0) / (pair[1].0 - pair[0].0);
            return Ok(pair[0].1 + weight * (pair[1].1 - pair[0].1));
        }
    }
    Ok(points[points.len() - 1].1)
}

/// Load and map the canonical source fixture under `docs/fixture/termsheet1.json`.
/// Market values retain their source provenance; placeholder/reference fields are not silently
/// treated as market-frozen observations.
pub fn load_termsheet1_fixture(path: impl AsRef<Path>) -> Result<LoadedFixture, RigaError> {
    let bytes = std::fs::read(path).map_err(|e| RigaError::new("fixture_io", e.to_string()))?;
    let root: Value = serde_json::from_slice(&bytes)
        .map_err(|e| RigaError::new("fixture_parse", e.to_string()))?;
    if root["fixture"].as_str() != Some("termsheet1") {
        return Err(RigaError::new(
            "fixture_identity",
            "expected fixture id termsheet1",
        ));
    }
    let assets = root["trade_economics"]["reference_assets"]
        .as_array()
        .ok_or_else(|| {
            RigaError::new(
                "fixture_missing_field",
                "trade_economics.reference_assets must be an array",
            )
        })?;
    if assets.len() != 2 {
        return Err(RigaError::new(
            "fixture_shape",
            "first build requires two reference assets",
        ));
    }
    let names: Vec<&str> = assets
        .iter()
        .map(|a| a["symbol"].as_str().unwrap_or(""))
        .collect();
    if names.iter().any(|n| n.is_empty()) {
        return Err(RigaError::new(
            "fixture_missing_field",
            "reference asset symbol is required",
        ));
    }
    let points = |asset: &str| -> Result<Vec<(f64, f64)>, RigaError> {
        let rows = root["market_data"]["vol_surfaces"][asset]
            .as_array()
            .ok_or_else(|| {
                RigaError::new(
                    "fixture_missing_field",
                    format!("vol surface for {asset} is required"),
                )
            })?;
        rows.iter()
            .map(|row| {
                let tenor = row["tenor"].as_str().unwrap_or("");
                let years = tenor_years(tenor).ok_or_else(|| {
                    RigaError::new(
                        "fixture_invalid_field",
                        format!("invalid vol tenor {tenor}"),
                    )
                })?;
                let vol = row["atm_implied_vol"].as_f64().ok_or_else(|| {
                    RigaError::new(
                        "fixture_missing_field",
                        format!("ATM vol missing for {asset}/{tenor}"),
                    )
                })?;
                Ok((years, vol))
            })
            .collect()
    };
    let vol0 = points(names[0])?;
    let vol1 = points(names[1])?;
    let as_of = root["as_of"].as_str().unwrap_or("");
    let final_fixing = root["trade_economics"]["final_fixing_date"]
        .as_str()
        .ok_or_else(|| {
            RigaError::new(
                "fixture_missing_field",
                "trade_economics.final_fixing_date is required",
            )
        })?;
    let (t, steps) = remaining_business_steps(as_of, final_fixing)?;
    if steps == 0 {
        return Err(RigaError::new(
            "fixture_invalid_date",
            "maturity interval has no weekday observations",
        ));
    }
    let vol0 = interpolate_curve(&vol0, t)?;
    let vol1 = interpolate_curve(&vol1, t)?;
    let discount_rows = root["market_data"]["discount_curve"]["points"]
        .as_array()
        .ok_or_else(|| {
            RigaError::new(
                "fixture_missing_field",
                "discount curve points are required",
            )
        })?;
    let discount_curve = discount_rows
        .iter()
        .map(|row| {
            let tenor = row["tenor"].as_str().and_then(tenor_years).ok_or_else(|| {
                RigaError::new("fixture_invalid_field", "discount curve tenor is invalid")
            })?;
            let rate = row["zero_rate"].as_f64().ok_or_else(|| {
                RigaError::new(
                    "fixture_missing_field",
                    "discount curve zero_rate is required",
                )
            })?;
            Ok((tenor, rate))
        })
        .collect::<Result<Vec<_>, RigaError>>()?;
    let risk_free_rate = interpolate_curve(&discount_curve, t)?;
    let trade = &root["trade_economics"];
    let run = &root["run"];
    let schedule = trade["schedule"].as_array().ok_or_else(|| {
        RigaError::new(
            "fixture_missing_field",
            "trade_economics.schedule must be an array",
        )
    })?;
    let future_coupons: Vec<&Value> = schedule
        .iter()
        .filter(|row| row["observation"].as_str().is_some_and(|date| date > as_of))
        .collect();
    if future_coupons.is_empty() {
        return Err(RigaError::new(
            "fixture_invalid_schedule",
            "no future coupon observations remain after as_of",
        ));
    }
    let fixed_first_coupon = future_coupons[0]["coupon_type"].as_str() == Some("fixed");
    let call_start = trade["callable"]["period_start"].as_str().ok_or_else(|| {
        RigaError::new(
            "fixture_missing_field",
            "trade_economics.callable.period_start is required",
        )
    })?;
    let call_start_step = if call_start <= as_of {
        0
    } else {
        weekday_steps(as_of, call_start)?
    };
    // Current as-of marks (captured). When absent, fall back to the pinned initial fixings.
    let quoted_spots: Vec<f64> = names
        .iter()
        .map(|n| {
            root["market_data"]["as_of_spots"][*n]
                .as_f64()
                .or_else(|| root["market_data"]["initial_fixings"]["spots"][*n].as_f64())
                .unwrap_or(f64::NAN)
        })
        .collect();
    // Optional full implied-vol grids → Dupire local vol (one per factor).
    let local_vol = match root["market_data"]["local_vol_surfaces"].as_object() {
        Some(surfaces) if !surfaces.is_empty() => {
            let mut grids = Vec::with_capacity(names.len());
            for name in &names {
                let node = &surfaces[*name];
                let rows = node["maturities"].as_array().ok_or_else(|| {
                    RigaError::new(
                        "fixture_missing_field",
                        format!("local_vol_surfaces.{name}.maturities is required"),
                    )
                })?;
                let raw_strikes = node["strikes"].as_array().ok_or_else(|| {
                    RigaError::new(
                        "fixture_missing_field",
                        format!("local_vol_surfaces.{name}.strikes is required"),
                    )
                })?;
                let raw_vols = node["vols"].as_array().ok_or_else(|| {
                    RigaError::new(
                        "fixture_missing_field",
                        format!("local_vol_surfaces.{name}.vols is required"),
                    )
                })?;
                let mut with_tenor: Vec<(f64, Vec<f64>)> = Vec::with_capacity(rows.len());
                for (k, row) in rows.iter().enumerate() {
                    let date = row.as_str().ok_or_else(|| {
                        RigaError::new("fixture_invalid_field", "local vol maturity must be a date")
                    })?;
                    let years = (civil_day_number(date)? - civil_day_number(as_of)?) as f64 / 365.0;
                    let vols = raw_vols
                        .get(k)
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            RigaError::new(
                                "fixture_invalid_field",
                                "local vol row count must match maturities",
                            )
                        })?
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(f64::NAN))
                        .collect::<Vec<_>>();
                    with_tenor.push((years, vols));
                }
                with_tenor
                    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                grids.push(ImpliedVolGrid {
                    maturities_years: with_tenor.iter().map(|r| r.0).collect(),
                    strikes: raw_strikes
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(f64::NAN))
                        .collect(),
                    vols: with_tenor.into_iter().map(|r| r.1).collect(),
                });
            }
            Some(LocalVolSpec { grids })
        }
        _ => None,
    };
    let request = PriceRequest {
        local_vol,
        trade: PhoenixTrade {
            trade_id: trade["trade_id"]
                .as_str()
                .unwrap_or("termsheet1")
                .to_owned(),
            initial_spots: names
                .iter()
                .map(|n| {
                    root["market_data"]["initial_fixings"]["spots"][*n]
                        .as_f64()
                        .unwrap_or(f64::NAN)
                })
                .collect(),
            maturity_years: t,
            steps,
            strike: number(
                &Value::Object(trade["barriers"].as_object().cloned().unwrap_or_default()),
                "/exercise/level_pct",
            )?,
            knock_in_barrier: number(
                &Value::Object(trade["barriers"].as_object().cloned().unwrap_or_default()),
                "/knock_in/level_pct",
            )?,
            knock_out_barrier: trade["barriers"]["call"]["level_pct"].as_f64(),
            coupon_barrier: trade["barriers"]["coupon"]["level_pct"].as_f64(),
            coupon_rate: trade["coupon"]["rate_per_period"].as_f64(),
            memory_coupon: trade["coupon"]["memory"].as_bool().unwrap_or(false),
            coupon_periods: Some(future_coupons.len()),
            fixed_first_coupon,
            call_start_step: Some(call_start_step),
        },
        market: MarketSnapshot {
            as_of: root["market_data"]["as_of"]
                .as_str()
                .unwrap_or("")
                .to_owned(),
            spots: quoted_spots,
            vols: vec![vol0, vol1],
            risk_free_rate,
            dividend_yields: names
                .iter()
                .map(|n| {
                    root["market_data"]["dividend_yields"]["annual_continuous"][*n]
                        .as_f64()
                        .unwrap_or(f64::NAN)
                })
                .collect(),
            correlation: serde_json::from_value(
                root["market_data"]["correlation_matrix"]["values"].clone(),
            )
            .map_err(|e| RigaError::new("fixture_parse", e.to_string()))?,
        },
        config: SimulationConfig {
            paths: run["paths"].as_u64().unwrap_or(30_000) as usize,
            path_block_size: run["path_block_size"].as_u64().unwrap_or(4_096) as usize,
            seed: run["seed"].as_u64().unwrap_or(1_729),
            trace_level: TraceLevel::Aggregated,
        },
        as_of: as_of.to_owned(),
    };
    let expected = FixtureExpected {
        put_pv: number(&root, "/expected/put/value")?,
        relative_tolerance: number(&root, "/expected/put/tol_rel")?,
        ki_probability: number(&root, "/expected/ki_prob/value")?,
        ki_probability_tolerance: number(&root, "/expected/ki_prob/tol_abs")?,
    };
    let has_local_vol = request.local_vol.is_some();
    Ok(LoadedFixture {
        name: "termsheet1".into(),
        request,
        expected,
        market_provenance: root["provenance"]["market_data.vol_surfaces"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        market_is_pinned: root["to_capture"]
            .as_array()
            .is_some_and(|items| items.is_empty())
            && root["provenance"].as_object().is_some_and(|items| {
                items.values().all(|v| {
                    v.as_str()
                        .is_some_and(|s| s.starts_with("pinned") || s.starts_with("captured"))
                })
            }),
        market_quality_note: if has_local_vol {
            "Captured fina-risk market: current as-of marks, full implied smile calibrated to Dupire local vol, USD discount curve, dividends, resolved correlation, and issuer-pinned coupon floor.".to_owned()
        } else {
            "Captured as-of market and issuer economics are loaded, but no full local-vol surface is present; pricing uses a flat-ATM GBM proxy rather than the source-declared Dupire model.".to_owned()
        },
        source_model: root["model"]["process"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
        engine_model: if has_local_vol {
            "dupire_local_vol_calibrated_cpu".to_owned()
        } else {
            "gbm_flat_atm_proxy_from_initial_fixings".to_owned()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_fixture() -> LoadedFixture {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixture/termsheet1.json");
        load_termsheet1_fixture(path).unwrap()
    }

    #[test]
    fn source_market_inputs_and_trade_economics_are_mapped() {
        let loaded = source_fixture();
        let request = loaded.request;
        assert_eq!(request.market.spots, vec![267.885, 258.355]);
        assert!((request.market.vols[0] - 0.45619).abs() < 0.002);
        assert!((request.market.vols[1] - 0.35599).abs() < 0.002);
        assert!((request.market.risk_free_rate - 0.038825).abs() < 0.0005);
        assert_eq!(request.market.dividend_yields, vec![0.0, 0.002979]);
        assert_eq!(request.market.correlation[0][1], 0.4593248180905);
        assert_eq!(request.trade.initial_spots, vec![239.97, 260.0]);
        assert_eq!(request.trade.strike, 0.78);
        assert_eq!(request.trade.knock_in_barrier, 0.70);
        assert_eq!(request.trade.knock_out_barrier, Some(1.10));
        assert_eq!(request.trade.coupon_barrier, Some(0.1));
        assert_eq!(request.trade.coupon_rate, Some(0.009642));
        assert!(request.trade.memory_coupon);
        assert_eq!(request.trade.coupon_periods, Some(5));
        assert!(!request.trade.fixed_first_coupon);
        assert_eq!(request.trade.call_start_step, Some(0));
        assert!(loaded.market_is_pinned);
        assert_eq!(loaded.source_model, "dupire_local_vol");
        assert_eq!(loaded.engine_model, "dupire_local_vol_calibrated_cpu");
        assert!(
            loaded
                .market_quality_note
                .contains("Captured fina-risk market")
        );
        let lv = request.local_vol.expect("captured smile must be present");
        assert_eq!(lv.grids.len(), 2);
        assert_eq!(lv.grids[0].maturities_years.len(), 10);
        assert_eq!(lv.grids[0].strikes.len(), 21);
        assert_eq!(lv.grids[0].vols.len(), 10);
        assert_eq!(lv.grids[0].vols[0].len(), 21);
    }

    #[test]
    fn calibrated_local_vol_matches_the_reference_grid() {
        let request = source_fixture().request;
        let lv = crate::build_local_vol(&request).unwrap().unwrap();
        // Reference local-vol points from the upstream calibration pipeline at the
        // fixture horizon (T = 0.40274, ADBE/AMZN). Tolerance covers the rate-forward
        // convention difference (fixture-interpolated vs first-pillar rate).
        let cases = [
            (0usize, 180.0, 0.467727),
            (0, 268.0, 0.430912),
            (1, 240.0, 0.346527),
            (1, 268.0, 0.352650),
        ];
        for (factor, s, expected) in cases {
            let got = lv[factor].sigma(0.40274, s);
            assert!(
                (got - expected).abs() < 3.0e-3,
                "factor {factor} sigma(0.40274, {s}) = {got}, reference {expected}"
            );
        }
    }

    #[test]
    fn date_conversion_counts_actual_days_and_weekdays() {
        let (years, weekdays) = remaining_business_steps("2026-09-07", "2027-02-01").unwrap();
        assert!((years - 147.0 / 365.0).abs() < f64::EPSILON);
        assert!(weekdays > 100 && weekdays < 110);
        assert_eq!(
            civil_day_number("2027-02-01").unwrap() - civil_day_number("2026-09-07").unwrap(),
            147
        );
    }
}

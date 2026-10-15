use riga_core::{Layout, Shape3, Tensor3};
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let shape = Shape3 {
        paths: 4096,
        times: 64,
        factors: 3,
    };
    let mut aos = Tensor3::zeros_with_layout(shape, Layout::PathTimeFactor).unwrap();
    let mut soa = Tensor3::zeros_with_layout(shape, Layout::FactorMajor).unwrap();
    for p in 0..shape.paths {
        for t in 0..shape.times {
            for f in 0..shape.factors {
                let value = ((p + 3 * t + 7 * f) % 101) as f32;
                aos.set(p, t, f, value).unwrap();
                soa.set(p, t, f, value).unwrap();
            }
        }
    }
    let measure = |tensor: &Tensor3| {
        let start = Instant::now();
        let mut checksum = 0.0_f64;
        for _ in 0..20 {
            for factor in 0..shape.factors {
                for time in 0..shape.times {
                    for path in 0..shape.paths {
                        checksum += f64::from(black_box(tensor.get(path, time, factor).unwrap()));
                    }
                }
            }
        }
        (start.elapsed(), checksum)
    };
    let (aos_time, aos_sum) = measure(&aos);
    let (soa_time, soa_sum) = measure(&soa);
    assert_eq!(aos_sum, soa_sum);
    println!(
        "paths={} times={} factors={} repetitions=20",
        shape.paths, shape.times, shape.factors
    );
    println!("PathTimeFactor/AoS: {:?}", aos_time);
    println!("FactorMajor/SoA: {:?}", soa_time);
    println!(
        "speedup: {:.2}x",
        aos_time.as_secs_f64() / soa_time.as_secs_f64()
    );
}

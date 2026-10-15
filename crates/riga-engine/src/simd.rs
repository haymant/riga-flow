//! Portable vectorised kernels (Phase 8).
//!
//! riga-flow pins `stable`, so `std::simd` is unavailable. These kernels use
//! `core::arch` on `x86_64` with a scalar fallback and are required to be
//! **bit-identical** to the scalar path (acceptance AC-C6). Only lane-independent,
//! correctly-rounded IEEE-754 operations (`min`/`mul`/`sub`/`div`) are vectorised,
//! so there is no reassociation and no cross-lane reduction.

/// Scalar reference: `out[i] = min(a[i], b[i])`.
pub fn worst_of_two_scalar(a: &[f32], b: &[f32], out: &mut [f32]) {
    assert_eq!(a.len(), b.len(), "worst-of inputs must be the same length");
    assert_eq!(
        a.len(),
        out.len(),
        "worst-of output must match input length"
    );
    for i in 0..a.len() {
        out[i] = a[i].min(b[i]);
    }
}

/// SSE2 worst-of (4 lanes).
#[cfg(target_arch = "x86_64")]
fn worst_of_two_sse2(a: &[f32], b: &[f32], out: &mut [f32]) {
    use core::arch::x86_64::{_mm_loadu_ps, _mm_min_ps, _mm_storeu_ps};
    let n = a.len();
    let mut i = 0;
    // SAFETY: the loop guards `i + 4 <= n`; pointers stay in bounds.
    unsafe {
        while i + 4 <= n {
            let va = _mm_loadu_ps(a.as_ptr().add(i));
            let vb = _mm_loadu_ps(b.as_ptr().add(i));
            _mm_storeu_ps(out.as_mut_ptr().add(i), _mm_min_ps(va, vb));
            i += 4;
        }
    }
    for j in i..n {
        out[j] = a[j].min(b[j]);
    }
}

/// AVX2 worst-of (8 lanes).
///
/// # Safety
/// Caller must ensure `is_x86_feature_detected!("avx2")` is true on this CPU.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn worst_of_two_avx2(a: &[f32], b: &[f32], out: &mut [f32]) {
    use core::arch::x86_64::{_mm256_loadu_ps, _mm256_min_ps, _mm256_storeu_ps};
    let n = a.len();
    let mut i = 0;
    while i + 8 <= n {
        let va = unsafe { _mm256_loadu_ps(a.as_ptr().add(i)) };
        let vb = unsafe { _mm256_loadu_ps(b.as_ptr().add(i)) };
        unsafe { _mm256_storeu_ps(out.as_mut_ptr().add(i), _mm256_min_ps(va, vb)) };
        i += 8;
    }
    for j in i..n {
        out[j] = a[j].min(b[j]);
    }
}

/// Vectorised worst-of over two performance slices.
///
/// Bit-identical to [`worst_of_two_scalar`] for finite inputs. Picks AVX2 when the CPU
/// supports it, else SSE2.
#[cfg(target_arch = "x86_64")]
pub fn worst_of_two(a: &[f32], b: &[f32], out: &mut [f32]) {
    assert_eq!(a.len(), b.len(), "worst-of inputs must be the same length");
    assert_eq!(
        a.len(),
        out.len(),
        "worst-of output must match input length"
    );
    if std::is_x86_feature_detected!("avx2") {
        // SAFETY: guarded by runtime feature detection.
        unsafe { worst_of_two_avx2(a, b, out) };
    } else {
        worst_of_two_sse2(a, b, out);
    }
}

#[cfg(not(target_arch = "x86_64"))]
pub fn worst_of_two(a: &[f32], b: &[f32], out: &mut [f32]) {
    worst_of_two_scalar(a, b, out);
}

/// Whether the build uses a vectorised worst-of kernel.
pub const IS_VECTORISED: bool = cfg!(target_arch = "x86_64");

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn vectorised_worst_of_is_bit_identical_to_scalar() {
        let mut rng = ChaCha8Rng::seed_from_u64(2026);
        for len in [0usize, 1, 3, 4, 7, 8, 4096, 10_001] {
            let a: Vec<f32> = (0..len).map(|_| rng.random_range(0.2..2.0)).collect();
            let b: Vec<f32> = (0..len).map(|_| rng.random_range(0.2..2.0)).collect();
            let mut scalar = vec![0.0f32; len];
            let mut vector = vec![0.0f32; len];
            worst_of_two_scalar(&a, &b, &mut scalar);
            worst_of_two(&a, &b, &mut vector);
            for i in 0..len {
                assert_eq!(
                    scalar[i].to_bits(),
                    vector[i].to_bits(),
                    "len {len} index {i}"
                );
            }
        }
    }
}

//! An in-place radix-2 complex FFT, enough for the level analyser's 1024 and 2048 points.
//!
//! Precision is f64 throughout; the analyser rounds to f32 where the Web Audio `AnalyserNode` keeps
//! floats (see `levels`).

// Used by the level analyser, which the ring (WP5) reads.
#![allow(dead_code)]

use std::f64::consts::PI;

pub struct Fft {
    n: usize,
    /// `e^(-2πik/n)` for k in 0..n/2.
    twiddles: Vec<(f64, f64)>,
    /// The bit-reversal permutation.
    bitrev: Vec<usize>,
}

impl Fft {
    /// `n` must be a power of two, at least 2.
    pub fn new(n: usize) -> Self {
        assert!(
            n >= 2 && n.is_power_of_two(),
            "FFT size {n} is not a power of two"
        );
        let bits = n.trailing_zeros();
        let bitrev = (0..n)
            .map(|i| i.reverse_bits() >> (usize::BITS - bits))
            .collect();
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos(), a.sin())
            })
            .collect();
        Self {
            n,
            twiddles,
            bitrev,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// The forward transform `X[k] = Σ x[j]·e^(-2πijk/n)`, unnormalised, in place.
    pub fn forward(&self, re: &mut [f64], im: &mut [f64]) {
        let n = self.n;
        assert!(re.len() == n && im.len() == n);
        for i in 0..n {
            let j = self.bitrev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let half = size / 2;
            let step = n / size;
            for start in (0..n).step_by(size) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * step];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            size *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(x: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let n = x.len();
        (0..n)
            .map(|k| {
                x.iter()
                    .enumerate()
                    .fold((0.0, 0.0), |(sr, si), (j, &(xr, xi))| {
                        let a = -2.0 * PI * (j * k) as f64 / n as f64;
                        (
                            sr + xr * a.cos() - xi * a.sin(),
                            si + xr * a.sin() + xi * a.cos(),
                        )
                    })
            })
            .collect()
    }

    #[test]
    fn matches_the_naive_dft() {
        let mut seed = 7u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f64 / (1u32 << 24) as f64 * 2.0 - 1.0
        };
        for n in [2, 4, 16, 64, 256] {
            let x: Vec<(f64, f64)> = (0..n).map(|_| (rnd(), rnd())).collect();
            let want = naive(&x);
            let fft = Fft::new(n);
            let mut re: Vec<f64> = x.iter().map(|v| v.0).collect();
            let mut im: Vec<f64> = x.iter().map(|v| v.1).collect();
            fft.forward(&mut re, &mut im);
            for k in 0..n {
                assert!(
                    (re[k] - want[k].0).abs() < 1e-9 && (im[k] - want[k].1).abs() < 1e-9,
                    "n={n} k={k}: ({}, {}) vs {:?}",
                    re[k],
                    im[k],
                    want[k]
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "not a power of two")]
    fn a_size_that_is_not_a_power_of_two_is_refused() {
        Fft::new(1000);
    }
}

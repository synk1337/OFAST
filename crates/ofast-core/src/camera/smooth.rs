//! Smoothing a view (M7): the first-person camera without the mouse's jitter, still exactly on the aim where the
//! player shoots.
//!
//! A Whittaker smoother (E. T. Whittaker, "On a New Method of Graduation", 1923; P. H. C. Eilers, "A Perfect
//! Smoother", 2003): of all the curves `z` through the samples `y`, the one that minimises
//! `Σ wᵢ (zᵢ − yᵢ)² + λ Σ (zᵢ₋₁ − 2 zᵢ + zᵢ₊₁)²`, a balance between staying on the samples and bending little. Its
//! solution is one banded linear system, `(W + λ DᵀD) z = W y` (D: second differences), solved here in one pass
//! (a pentadiagonal Cholesky factorisation). A sample with a very large weight is pinned: the curve goes through it,
//! and bends least around it. The view is pinned on the ticks around each shot, so the crosshair is where the
//! bullets, tracers and hits (all built from the original aim) are.
//!
//! The strength `λ` is in ticks: a wiggle much shorter than about `2π λ^¼` ticks is smoothed away, a longer move
//! kept ([`lambda`]: strength 0.5 → λ ≈ 178, about 0.36 s at 64 ticks a second).

use super::{unwrap_degrees, wrap_degrees};

/// The weight of a pinned sample (others weigh 1).
pub const PINNED: f64 = 1e9;

/// How many ticks on each side of a shot's tick the view is pinned.
pub const PIN_AROUND_SHOT: u32 = 1;

/// The smoother's `λ` for a strength from 0 (none) to 1 (strong: a wiggle under about a second goes):
/// `10^(0.5 + 3.5 s)`; 0 for a strength of 0 or less.
pub fn lambda(strength: f64) -> f64 {
    if strength <= 0.0 {
        0.0
    } else {
        10f64.powf(0.5 + 3.5 * strength.min(1.0))
    }
}

/// The Whittaker smoother of `values` with per-sample `weights` (same length; > 0) and `lambda`: see the module
/// docs. With fewer than three samples or `lambda` 0 the values come back as they are.
pub fn whittaker(values: &[f64], weights: &[f64], lambda: f64) -> Vec<f64> {
    let n = values.len();
    assert_eq!(n, weights.len(), "one weight per value");
    if n < 3 || lambda <= 0.0 {
        return values.to_vec();
    }
    // The symmetric pentadiagonal matrix W + λ DᵀD: its diagonal and the two bands right of it. Row k of D is
    // (1, −2, 1) on columns k, k + 1, k + 2.
    let mut d0: Vec<f64> = weights.to_vec();
    let mut d1 = vec![0.0; n - 1];
    let mut d2 = vec![0.0; n - 2];
    for k in 0..n - 2 {
        d0[k] += lambda;
        d0[k + 1] += 4.0 * lambda;
        d0[k + 2] += lambda;
        d1[k] -= 2.0 * lambda;
        d1[k + 1] -= 2.0 * lambda;
        d2[k] += lambda;
    }
    // Cholesky, A = L Lᵀ, L lower triangular with two bands under its diagonal (l0 diagonal, l1, l2 below).
    let mut l0 = vec![0.0; n];
    let mut l1 = vec![0.0; n];
    let mut l2 = vec![0.0; n];
    for i in 0..n {
        if i >= 2 {
            l2[i] = d2[i - 2] / l0[i - 2];
        }
        if i >= 1 {
            let above = if i >= 2 { l2[i] * l1[i - 1] } else { 0.0 };
            l1[i] = (d1[i - 1] - above) / l0[i - 1];
        }
        let diagonal = d0[i] - l1[i] * l1[i] - l2[i] * l2[i];
        l0[i] = diagonal.max(f64::MIN_POSITIVE).sqrt();
    }
    // L y = W v, then Lᵀ z = y.
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = weights[i] * values[i];
        if i >= 1 {
            s -= l1[i] * y[i - 1];
        }
        if i >= 2 {
            s -= l2[i] * y[i - 2];
        }
        y[i] = s / l0[i];
    }
    let mut z = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        if i + 1 < n {
            s -= l1[i + 1] * z[i + 1];
        }
        if i + 2 < n {
            s -= l2[i + 2] * z[i + 2];
        }
        z[i] = s / l0[i];
    }
    z
}

/// A view's `(pitch, yaw)` (degrees, Source's) smoothed with `lambda`, pinned where `pinned` is set. Yaw is
/// unwrapped first (a turn across ±180° stays small) and wrapped back into (−180, 180] after.
pub fn smooth_view(view: &[[f64; 2]], pinned: &[bool], lambda: f64) -> Vec<[f64; 2]> {
    let weights: Vec<f64> = pinned
        .iter()
        .map(|&p| if p { PINNED } else { 1.0 })
        .collect();
    let pitch: Vec<f64> = view.iter().map(|v| v[0]).collect();
    let mut yaw: Vec<f64> = view.iter().map(|v| v[1]).collect();
    unwrap_degrees(&mut yaw);
    let pitch = whittaker(&pitch, &weights, lambda);
    let yaw = whittaker(&yaw, &weights, lambda);
    pitch
        .into_iter()
        .zip(yaw)
        .map(|(p, y)| [p, wrap_degrees(y)])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic jitter in −1..1.
    fn jitter(i: usize) -> f64 {
        let x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 11;
        (x as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }

    #[test]
    fn a_line_stays_a_line() {
        let values: Vec<f64> = (0..50).map(|i| 3.0 * i as f64 - 7.0).collect();
        let z = whittaker(&values, &vec![1.0; 50], 1000.0);
        for (a, b) in values.iter().zip(&z) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn jitter_is_smoothed_and_pins_hold() {
        let n = 400;
        let truth: Vec<f64> = (0..n).map(|i| (i as f64 * 0.02).sin() * 20.0).collect();
        let noisy: Vec<f64> = truth
            .iter()
            .enumerate()
            .map(|(i, t)| t + jitter(i))
            .collect();
        let mut weights = vec![1.0; n];
        weights[200] = PINNED;
        let z = whittaker(&noisy, &weights, lambda(0.5));
        let error = |a: &[f64]| {
            a.iter()
                .zip(&truth)
                .map(|(x, t)| (x - t).powi(2))
                .sum::<f64>()
                / n as f64
        };
        assert!(
            error(&z) < error(&noisy) / 4.0,
            "{} vs {}",
            error(&z),
            error(&noisy)
        );
        assert!((z[200] - noisy[200]).abs() < 1e-5);
    }

    #[test]
    fn yaw_across_the_seam_smooths_the_short_way() {
        let view: Vec<[f64; 2]> = (0..60)
            .map(|i| [0.0, wrap_degrees(170.0 + i as f64 * 0.5)])
            .collect();
        let pinned = vec![false; 60];
        let smooth = smooth_view(&view, &pinned, lambda(0.8));
        for (a, b) in view.iter().zip(&smooth) {
            assert!(wrap_degrees(a[1] - b[1]).abs() < 1e-6, "{a:?} {b:?}");
            assert!(b[1] > -180.0 && b[1] <= 180.0);
        }
    }

    #[test]
    fn no_strength_no_change() {
        assert_eq!(lambda(0.0), 0.0);
        let v = [1.0, 5.0, 2.0, 8.0];
        assert_eq!(whittaker(&v, &[1.0; 4], 0.0), v.to_vec());
    }
}

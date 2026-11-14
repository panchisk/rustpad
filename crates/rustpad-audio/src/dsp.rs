//! Offline DSP used by import (loudness), the sound editor and the recorder.
//!
//! All functions operate on interleaved `f32` samples. "dB" values are dBFS.
//! Guard against `0.0`/NaN: silence is `-120.0` dB, and gain is clamped to
//! `1e-4 ..= 64.0`.

use crate::resample::resample_all;

/// Reported level for digital silence / unusable input.
const FLOOR_DB: f32 = -120.0;
/// Lowest gain the automatic helpers will apply.
const MIN_GAIN: f32 = 1e-4;
/// Highest gain the automatic helpers will apply.
const MAX_GAIN: f32 = 64.0;
/// Default silence threshold for [`trim_silence`].
const DEFAULT_TRIM_DB: f32 = -50.0;

#[inline]
fn linear_to_db(v: f32) -> f32 {
    if !v.is_finite() || v <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * v.log10()).max(FLOOR_DB)
    }
}

#[inline]
fn db_to_linear(db: f32) -> f32 {
    if !db.is_finite() {
        1.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Soft limiter: transparent (identity) up to 0 dBFS, `tanh`-shaped above.
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= 1.0 {
        x
    } else {
        // Normalised so the curve is continuous at 0 dBFS (tanh(1)/tanh(1) == 1).
        x.signum() * (a.tanh() / 1.0f32.tanh())
    }
}

/// Peak level in dBFS.
pub fn peak_db(samples: &[f32]) -> f32 {
    let mut peak = 0.0f32;
    for s in samples {
        let a = s.abs();
        if a.is_finite() && a > peak {
            peak = a;
        }
    }
    linear_to_db(peak)
}

/// RMS level in dBFS (all channels combined).
pub fn rms_db(samples: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for s in samples {
        if s.is_finite() {
            let v = *s as f64;
            sum += v * v;
            n += 1;
        }
    }
    if n == 0 || sum <= 0.0 {
        return FLOOR_DB;
    }
    let rms = (sum / n as f64).sqrt() as f32;
    linear_to_db(rms)
}

/// Linear gain that brings `samples` to `target_db` RMS, clamped to a sane range.
/// Returns 1.0 for silence.
pub fn gain_for_target(samples: &[f32], target_db: f32) -> f32 {
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for s in samples {
        if s.is_finite() {
            let v = *s as f64;
            sum += v * v;
            n += 1;
        }
    }
    if n == 0 || sum <= 0.0 || !target_db.is_finite() {
        return 1.0;
    }
    let rms = (sum / n as f64).sqrt() as f32;
    if rms <= 1e-6 {
        return 1.0;
    }
    let gain = db_to_linear(target_db) / rms;
    if !gain.is_finite() {
        1.0
    } else {
        gain.clamp(MIN_GAIN, MAX_GAIN)
    }
}

/// Applies gain in place with a soft limiter (`tanh`) above 0 dBFS.
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if !gain.is_finite() {
        return;
    }
    for s in samples.iter_mut() {
        if !s.is_finite() {
            *s = 0.0;
            continue;
        }
        *s = soft_limit(*s * gain);
    }
}

/// Peak-normalises in place to `target_db` and returns the applied gain.
pub fn normalize_peak(samples: &mut [f32], target_db: f32) -> f32 {
    let mut peak = 0.0f32;
    for s in samples.iter() {
        let a = s.abs();
        if a.is_finite() && a > peak {
            peak = a;
        }
    }
    if peak <= 1e-6 || !target_db.is_finite() {
        return 1.0;
    }
    let gain = (db_to_linear(target_db) / peak).clamp(MIN_GAIN, MAX_GAIN);
    if !gain.is_finite() {
        return 1.0;
    }
    for s in samples.iter_mut() {
        if s.is_finite() {
            *s *= gain;
        } else {
            *s = 0.0;
        }
    }
    gain
}

/// Trims leading/trailing silence below `threshold_db` (default -50 dB),
/// keeping `pad_ms` of padding. Returns `(start_frame, end_frame)` removed counts.
pub fn trim_silence(
    samples: &mut Vec<f32>,
    channels: u16,
    sample_rate: u32,
    threshold_db: f32,
    pad_ms: u32,
) -> (usize, usize) {
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    if frames == 0 || sample_rate == 0 {
        return (0, 0);
    }
    let threshold_db = if threshold_db.is_finite() {
        threshold_db
    } else {
        DEFAULT_TRIM_DB
    };
    let threshold = db_to_linear(threshold_db).max(1e-7);

    let frame_peak = |f: usize| -> f32 {
        let base = f * ch;
        let mut peak = 0.0f32;
        for c in 0..ch {
            let a = samples[base + c].abs();
            if a.is_finite() && a > peak {
                peak = a;
            }
        }
        peak
    };

    let mut first = None;
    let mut last = None;
    for f in 0..frames {
        if frame_peak(f) >= threshold {
            if first.is_none() {
                first = Some(f);
            }
            last = Some(f);
        }
    }

    let (Some(first), Some(last)) = (first, last) else {
        // Nothing above the threshold: never delete the whole buffer.
        return (0, 0);
    };

    let pad = (pad_ms as u64 * sample_rate as u64 / 1000) as usize;
    let start = first.saturating_sub(pad);
    let end = (last + 1 + pad).min(frames);
    if start >= end {
        return (0, 0);
    }

    // Remove the tail first so the head indices stay valid.
    samples.truncate(end * ch);
    samples.drain(..start * ch);
    (start, frames - end)
}

/// Linear fade-in over `ms` milliseconds.
pub fn fade_in(samples: &mut [f32], channels: u16, sample_rate: u32, ms: u32) {
    let ch = channels.max(1) as usize;
    if ms == 0 || sample_rate == 0 {
        return;
    }
    let frames = samples.len() / ch;
    let fade = ((ms as u64 * sample_rate as u64 / 1000) as usize)
        .max(1)
        .min(frames);
    if fade == 0 {
        return;
    }
    for f in 0..fade {
        let g = if fade <= 1 {
            1.0
        } else {
            f as f32 / (fade - 1) as f32
        };
        for c in 0..ch {
            samples[f * ch + c] *= g;
        }
    }
}

pub fn fade_out(samples: &mut [f32], channels: u16, sample_rate: u32, ms: u32) {
    let ch = channels.max(1) as usize;
    if ms == 0 || sample_rate == 0 {
        return;
    }
    let frames = samples.len() / ch;
    let fade = ((ms as u64 * sample_rate as u64 / 1000) as usize)
        .max(1)
        .min(frames);
    if fade == 0 {
        return;
    }
    let start = frames - fade;
    for f in start..frames {
        let remaining = frames - 1 - f;
        let g = if fade <= 1 {
            0.0
        } else {
            remaining as f32 / (fade - 1) as f32
        };
        for c in 0..ch {
            samples[f * ch + c] *= g;
        }
    }
}

/// Reverses frame order, preserving channel interleaving.
pub fn reverse(samples: &mut [f32], channels: u16) {
    let ch = channels.max(1) as usize;
    let frames = samples.len() / ch;
    if frames < 2 {
        return;
    }
    for f in 0..frames / 2 {
        let a = f * ch;
        let b = (frames - 1 - f) * ch;
        for c in 0..ch {
            samples.swap(a + c, b + c);
        }
    }
}

/// Extracts `[start_ms, end_ms)` as a new buffer.
pub fn crop(
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    start_ms: u64,
    end_ms: u64,
) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if sample_rate == 0 || end_ms <= start_ms {
        return Vec::new();
    }
    let frames = (samples.len() / ch) as u64;
    let start = (start_ms.saturating_mul(sample_rate as u64) / 1000).min(frames);
    let end = (end_ms.saturating_mul(sample_rate as u64) / 1000).min(frames);
    if end <= start {
        return Vec::new();
    }
    let a = start as usize * ch;
    let b = end as usize * ch;
    samples[a..b].to_vec()
}

/// Speed/pitch change by resampling (factor > 1 = faster/higher).
pub fn change_speed(samples: &[f32], channels: u16, sample_rate: u32, factor: f32) -> Vec<f32> {
    let factor = if factor.is_finite() {
        factor.clamp(0.1, 4.0)
    } else {
        1.0
    };
    if sample_rate == 0 || samples.is_empty() {
        return Vec::new();
    }
    if (factor - 1.0).abs() < 1e-6 {
        return samples.to_vec();
    }
    // Reading the source as if it had a higher rate makes the result shorter.
    let source_rate =
        ((sample_rate as f64 * factor as f64).round() as u64).clamp(1, u32::MAX as u64) as u32;
    resample_all(samples, source_rate, sample_rate, channels)
}

/// Applies gain in dB, then soft-clips.
pub fn boost(samples: &mut [f32], db: f32) {
    let db = if db.is_finite() { db } else { 0.0 };
    let gain = db_to_linear(db).clamp(MIN_GAIN, MAX_GAIN);
    apply_gain(samples, gain);
}

/// Karaoke: removes the centre channel of a stereo signal (L-R vocal cancellation).
pub fn karaoke(samples: &mut [f32], channels: u16) {
    if channels != 2 {
        return;
    }
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for frame in samples.chunks_exact_mut(2) {
        let l = frame[0];
        let r = frame[1];
        frame[0] = (l - r) * K;
        frame[1] = (r - l) * K;
    }
}

/// Mixes all channels to mono (length = frames).
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if ch == 1 {
        return samples.to_vec();
    }
    let frames = samples.len() / ch;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let base = f * ch;
        let mut sum = 0.0f32;
        for c in 0..ch {
            sum += samples[base + c];
        }
        out.push(sum / ch as f32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, frames: usize, freq: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn silence_floor() {
        assert_eq!(peak_db(&[]), -120.0);
        assert_eq!(rms_db(&[]), -120.0);
        assert_eq!(peak_db(&[0.0; 128]), -120.0);
        assert_eq!(rms_db(&[0.0; 128]), -120.0);
        assert_eq!(peak_db(&[0.0, f32::NAN, 0.0]), -120.0);
        assert_eq!(rms_db(&[f32::NAN, 0.0]), -120.0);
    }

    #[test]
    fn sine_levels() {
        let s = sine(48_000, 48_000, 440.0, 0.707);
        assert!((peak_db(&s) + 3.0).abs() < 0.1, "peak {}", peak_db(&s));
        // A sine with 0.707 peak has 0.5 RMS == -6 dB.
        assert!((rms_db(&s) + 6.02).abs() < 0.1, "rms {}", rms_db(&s));

        let full = sine(48_000, 48_000, 440.0, 1.0);
        assert!((rms_db(&full) + 3.01).abs() < 0.1, "rms {}", rms_db(&full));
    }

    #[test]
    fn gain_helpers() {
        let mut s = sine(48_000, 4_800, 440.0, 0.1);
        let gain = gain_for_target(&s, -6.0);
        assert!(gain > 1.0);
        apply_gain(&mut s, gain);
        assert!((rms_db(&s) + 6.0).abs() < 0.2);

        assert_eq!(gain_for_target(&[0.0; 16], -6.0), 1.0);
        assert!((1e-4..=64.0).contains(&gain_for_target(&[1e-9; 16], 0.0)));

        // Limiter keeps hot signals finite and bounded.
        let mut hot = vec![0.9f32, 3.0, -4.0];
        apply_gain(&mut hot, 4.0);
        assert!(hot.iter().all(|v| v.is_finite() && v.abs() <= 1.4));
        // Transparent below 0 dBFS.
        let mut quiet = vec![0.25f32, -0.5];
        apply_gain(&mut quiet, 1.0);
        assert_eq!(quiet, vec![0.25, -0.5]);
    }

    #[test]
    fn normalize_peak_reaches_target() {
        let mut s = sine(48_000, 4_800, 440.0, 0.2);
        let gain = normalize_peak(&mut s, -1.0);
        assert!(gain > 1.0);
        assert!((peak_db(&s) + 1.0).abs() < 0.05);
        assert_eq!(normalize_peak(&mut [0.0; 8], -1.0), 1.0);
    }

    #[test]
    fn trim_removes_padded_silence() {
        let ch = 1u16;
        let sr = 1_000u32;
        let mut s = vec![0.0f32; 1_000];
        s.extend(std::iter::repeat(0.5).take(500));
        s.extend(std::iter::repeat(0.0).take(1_000));
        let (head, tail) = trim_silence(&mut s, ch, sr, -50.0, 0);
        assert_eq!(head, 1_000);
        assert_eq!(tail, 1_000);
        assert_eq!(s.len(), 500);
        assert!(s.iter().all(|v| (*v - 0.5).abs() < 1e-6));

        let (head, tail) = trim_silence(&mut s, ch, sr, -50.0, 50);
        assert_eq!((head, tail), (0, 0));
        assert_eq!(s.len(), 500);

        // Fully silent buffers are never emptied.
        let mut silent = vec![0.0f32; 100];
        assert_eq!(trim_silence(&mut silent, ch, sr, -50.0, 10), (0, 0));
        assert_eq!(silent.len(), 100);
    }

    #[test]
    fn fades_hit_endpoints() {
        let sr = 1_000u32;
        let original = vec![0.8f32; 1_000];
        let mut s = original.clone();
        fade_in(&mut s, 1, sr, 100);
        assert!(s[0].abs() < 1e-6);
        assert!((s[99] - 0.8).abs() < 1e-6);
        assert!((s[100] - 0.8).abs() < 1e-6);
        assert!(s[50] > 0.3 && s[50] < 0.5);

        let mut s = original.clone();
        fade_out(&mut s, 1, sr, 100);
        assert!((s[0] - 0.8).abs() < 1e-6);
        assert!((s[900] - 0.8).abs() < 1e-6);
        assert!(s[999].abs() < 1e-6);

        // ms == 0 is a no-op.
        let mut s = original.clone();
        fade_in(&mut s, 1, sr, 0);
        fade_out(&mut s, 1, sr, 0);
        assert_eq!(s, original);
    }

    #[test]
    fn reverse_roundtrip() {
        let sr = 1_000u32;
        let mut s = vec![0.0f32, 1.0, 2.0, 3.0, 2.0, 1.0, 0.0];
        let palindrome = s.clone();
        reverse(&mut s, 1);
        assert_eq!(s, palindrome);
        reverse(&mut s, 1);
        assert_eq!(s, vec![0.0, 1.0, 2.0, 3.0, 2.0, 1.0, 0.0]);

        // Stereo frames stay together.
        let mut stereo = vec![0.0f32, 10.0, 1.0, 11.0, 2.0, 12.0];
        reverse(&mut stereo, 2);
        assert_eq!(stereo, vec![2.0, 12.0, 1.0, 11.0, 0.0, 10.0]);
        let _ = sr;
    }

    #[test]
    fn crop_clamps_and_validates() {
        let sr = 1_000u32;
        let s: Vec<f32> = (0..2_000).map(|i| i as f32).collect();
        assert_eq!(crop(&s, 1, sr, 500, 1_500).len(), 1_000);
        assert_eq!(crop(&s, 1, sr, 500, 1_500)[0], 500.0);
        assert!(crop(&s, 1, sr, 1_500, 500).is_empty());
        assert!(crop(&s, 1, sr, 3_000, 4_000).is_empty());
        assert_eq!(crop(&s, 1, sr, 1_500, 9_999).len(), 500);
    }

    #[test]
    fn change_speed_scales_length() {
        let sr = 48_000u32;
        let s = sine(sr, 48_000, 440.0, 0.5);
        let fast = change_speed(&s, 1, sr, 2.0);
        assert!((fast.len() as f64 - 24_000.0).abs() <= 2.0);
        let slow = change_speed(&s, 1, sr, 0.5);
        assert!((slow.len() as f64 - 96_000.0).abs() <= 2.0);
        // Factor is clamped, never panics or explodes.
        let clamped = change_speed(&s, 1, sr, 100.0);
        assert!((clamped.len() as f64 - 12_000.0).abs() <= 2.0);
        assert!(change_speed(&s, 1, sr, f32::NAN).len() == s.len());
    }

    #[test]
    fn boost_soft_clips() {
        let mut s = vec![0.5f32, -0.5];
        boost(&mut s, 6.0);
        assert!(s[0] > 0.9 && s[0] <= 1.5);
        assert!((s[0] + s[1]).abs() < 1e-6);

        let mut hot = vec![1.5f32];
        boost(&mut hot, 0.0);
        assert!(hot[0].is_finite() && hot[0] > 1.0 && hot[0] < 1.6);
    }

    #[test]
    fn karaoke_cancels_centre() {
        let mut s = vec![0.5f32, 0.5, 0.25, 0.25, -0.4, -0.4];
        karaoke(&mut s, 2);
        assert!(s.iter().all(|v| v.abs() < 1e-6));

        let mut wide = vec![0.5f32, -0.5];
        karaoke(&mut wide, 2);
        assert!((wide[0] - 0.7071).abs() < 1e-3);
        assert!((wide[1] + 0.7071).abs() < 1e-3);

        let mono = vec![0.5f32, 0.5];
        let mut mono2 = mono.clone();
        karaoke(&mut mono2, 1);
        assert_eq!(mono, mono2);
    }

    #[test]
    fn to_mono_averages() {
        let s = vec![0.5f32, -0.5, 1.0, 0.0, 0.0, 0.0];
        let mono = to_mono(&s, 2);
        assert_eq!(mono.len(), 3);
        assert_eq!(mono, vec![0.0, 0.5, 0.0]);
        assert_eq!(to_mono(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }
}

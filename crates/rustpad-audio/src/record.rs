//! Microphone recorder buffer.
//!
//! The engine feeds captured frames into [`Recorder`] until the caller stops it.
//! The recorder is intentionally allocation-light in `push` (reserves up front
//! from `max_secs`) and owns no threads.

use crate::types::DecodedSound;

/// Hard ceiling for `max_secs == 0` (60 minutes).
const MAX_UNLIMITED_SECS: u32 = 60 * 60;

pub struct Recorder {
    sample_rate: u32,
    channels: u16,
    samples: Vec<f32>,
    max_frames: u64,
    peak: f32,
    truncated: bool,
}

pub struct RecordedClip {
    pub audio: DecodedSound,
    /// True when `max_frames` was reached and later input was dropped.
    pub truncated: bool,
}

impl Recorder {
    /// `max_secs == 0` means unlimited (still capped at 60 minutes for safety).
    pub fn new(sample_rate: u32, channels: u16, max_secs: u32) -> Self {
        let sample_rate = sample_rate.max(1);
        let channels = channels.max(1);
        let secs = if max_secs == 0 {
            MAX_UNLIMITED_SECS
        } else {
            max_secs
        };
        let max_frames = sample_rate as u64 * secs as u64;

        // Reserve up front so `push` almost never has to grow the buffer. Absurd
        // `max_secs` values are capped to 2 GiB of samples to avoid OOM.
        const MAX_RESERVE_SAMPLES: u64 = 2 * 1024 * 1024 * 1024 / 4;
        let reserve_samples = max_frames
            .saturating_mul(channels as u64)
            .min(MAX_RESERVE_SAMPLES) as usize;
        let mut samples = Vec::new();
        let _ = samples.try_reserve_exact(reserve_samples);

        Recorder {
            sample_rate,
            channels,
            samples,
            max_frames,
            peak: 0.0,
            truncated: false,
        }
    }

    /// Appends interleaved frames, ignores input past the limit.
    pub fn push(&mut self, samples: &[f32]) {
        let ch = self.channels as usize;
        let recorded_frames = self.samples.len() / ch;
        let capacity_frames = self.max_frames.saturating_sub(recorded_frames as u64);
        let input_frames = (samples.len() / ch) as u64;

        if input_frames > capacity_frames {
            self.truncated = true;
        }

        let frames_to_take = capacity_frames.min(input_frames) as usize;
        let take = frames_to_take * ch;
        if take == 0 {
            return;
        }

        let accepted = &samples[..take];
        let mut peak = self.peak;
        for &s in accepted {
            if !s.is_finite() {
                continue;
            }
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        self.peak = peak;
        self.samples.extend_from_slice(accepted);
    }

    /// Largest absolute sample observed so far (for a level meter).
    pub fn peak(&self) -> f32 {
        self.peak
    }

    pub fn duration_ms(&self) -> u64 {
        let ch = self.channels as usize;
        let frames = (self.samples.len() / ch) as u64;
        frames.saturating_mul(1000) / self.sample_rate as u64
    }

    pub fn finish(self) -> RecordedClip {
        RecordedClip {
            audio: DecodedSound {
                sample_rate: self.sample_rate,
                channels: self.channels,
                samples: self.samples,
            },
            truncated: self.truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reports_duration() {
        let mut rec = Recorder::new(1_000, 1, 0);
        rec.push(&[0.1; 250]);
        rec.push(&[0.2; 250]);
        assert_eq!(rec.duration_ms(), 500);
        let clip = rec.finish();
        assert_eq!(clip.audio.sample_rate, 1_000);
        assert_eq!(clip.audio.channels, 1);
        assert_eq!(clip.audio.samples.len(), 500);
        assert!(!clip.truncated);
    }

    #[test]
    fn truncates_at_limit() {
        let mut rec = Recorder::new(1_000, 1, 1);
        rec.push(&[0.3; 750]);
        rec.push(&[0.3; 750]);
        assert!(rec.peak() > 0.0);
        let clip = rec.finish();
        assert!(clip.truncated);
        assert_eq!(clip.audio.samples.len(), 1_000);
        assert_eq!(clip.audio.duration_ms(), 1_000);
    }

    #[test]
    fn tracks_peak_post_clamp() {
        let mut rec = Recorder::new(1_000, 2, 1);
        // The 1 s / 2 ch buffer holds 1000 stereo frames (2000 samples).
        rec.push(&[0.5; 2_000]);
        rec.push(&[9.0; 4]);
        assert!((rec.peak() - 0.5).abs() < 1e-6);
        assert!(rec.finish().truncated);
    }

    #[test]
    fn unlimited_defaults_to_60_minutes() {
        let rec = Recorder::new(48_000, 1, 0);
        assert_eq!(rec.max_frames, 48_000 * 3_600);
        assert_eq!(rec.duration_ms(), 0);
        assert_eq!(rec.peak(), 0.0);
    }
}

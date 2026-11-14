//! Real-time mixer: plays several decoded sounds at once, resampling on the fly.
//!
//! The mixer is owned by an audio callback behind a mutex. `render` must not
//! allocate after warm-up (the finished-voice list is fixed-size).

use std::sync::Arc;

use crate::types::DecodedSound;

pub type VoiceId = u64;

const MAX_FINISHED: usize = 64;

struct Voice {
    id: VoiceId,
    /// Stereo interleaved source.
    data: Arc<DecodedSound>,
    /// Fractional read position in source frames.
    pos: f64,
    /// source_rate / output_rate
    step: f64,
    gain: f32,
    /// Current fade multiplier and per-frame delta (for stop / pause ramps).
    fade: f32,
    fade_step: f32,
    playing: bool,
    looping: bool,
    start_frame: f64,
    end_frame: f64,
    stop_requested: bool,
    /// Output frames to emit as silence before the voice starts (mic PTT delay).
    delay_frames: u64,
}

pub struct Mixer {
    sample_rate: u32,
    master: f32,
    voices: Vec<Voice>,
    next_id: VoiceId,
    finished: [VoiceId; MAX_FINISHED],
    finished_len: usize,
    last_started: Option<VoiceId>,
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Self {
        Mixer {
            sample_rate: sample_rate.max(8_000),
            master: 1.0,
            voices: Vec::with_capacity(8),
            next_id: 1,
            finished: [0; MAX_FINISHED],
            finished_len: 0,
            last_started: None,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = gain.clamp(0.0, 4.0);
    }

    pub fn master(&self) -> f32 {
        self.master
    }

    /// Adds a voice. `gain` is linear (normalisation already applied).
    pub fn play(
        &mut self,
        data: Arc<DecodedSound>,
        gain: f32,
        looping: bool,
        start_ms: u64,
    ) -> VoiceId {
        self.play_delayed(data, gain, looping, start_ms, 0)
    }

    /// Like [`Self::play`], but waits `delay_ms` before the first sample.
    pub fn play_delayed(
        &mut self,
        data: Arc<DecodedSound>,
        gain: f32,
        looping: bool,
        start_ms: u64,
        delay_ms: u32,
    ) -> VoiceId {
        let stereo = if data.channels == 2 {
            data
        } else {
            Arc::new(data.to_stereo())
        };
        let sr = stereo.sample_rate.max(1) as f64;
        let start_frame = (start_ms as f64 * sr / 1000.0).floor();
        let end_frame = stereo.frames() as f64;
        let id = self.next_id;
        self.next_id += 1;
        self.voices.push(Voice {
            id,
            data: stereo,
            pos: start_frame.min(end_frame),
            step: sr / self.sample_rate as f64,
            gain: gain.clamp(0.0, 8.0),
            fade: 1.0,
            fade_step: 0.0,
            playing: true,
            looping,
            start_frame,
            end_frame,
            stop_requested: false,
            delay_frames: (self.sample_rate as u64 * delay_ms as u64) / 1000,
        });
        self.last_started = Some(id);
        id
    }

    pub fn stop(&mut self, id: VoiceId) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
            v.stop_requested = true;
            v.playing = true;
        }
    }

    pub fn stop_all(&mut self) {
        for v in &mut self.voices {
            v.stop_requested = true;
            v.playing = true;
        }
    }

    pub fn pause_all(&mut self) {
        for v in &mut self.voices {
            v.playing = false;
        }
    }

    pub fn resume_all(&mut self) {
        for v in &mut self.voices {
            v.playing = true;
        }
    }

    pub fn is_paused(&self) -> bool {
        !self.voices.is_empty() && self.voices.iter().all(|v| !v.playing)
    }

    pub fn has_active(&self) -> bool {
        !self.voices.is_empty()
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn position_ms(&self, id: Option<VoiceId>) -> u64 {
        let id = id.or(self.last_started);
        if let Some(id) = id {
            if let Some(v) = self.voices.iter().find(|v| v.id == id) {
                return (v.pos / (v.data.sample_rate.max(1) as f64) * 1000.0) as u64;
            }
        }
        0
    }

    pub fn duration_ms(&self, id: Option<VoiceId>) -> u64 {
        let id = id.or(self.last_started);
        id.and_then(|id| self.voices.iter().find(|v| v.id == id))
            .map(|v| v.data.duration_ms())
            .unwrap_or(0)
    }

    pub fn last_started(&self) -> Option<VoiceId> {
        self.last_started
    }

    pub fn seek(&mut self, id: VoiceId, ms: u64) {
        if let Some(v) = self.voices.iter_mut().find(|v| v.id == id) {
            let sr = v.data.sample_rate.max(1) as f64;
            v.pos = ((ms as f64 * sr / 1000.0) as f64).min(v.end_frame);
        }
    }

    pub fn take_finished(&mut self) -> Vec<VoiceId> {
        let out = self.finished[..self.finished_len].to_vec();
        self.finished_len = 0;
        out
    }

    fn push_finished(&mut self, id: VoiceId) {
        if self.finished_len < MAX_FINISHED {
            self.finished[self.finished_len] = id;
            self.finished_len += 1;
        }
    }

    /// Renders `frames` stereo frames into `out` (len >= frames*2), replacing content.
    pub fn render(&mut self, frames: usize, out: &mut [f32]) {
        let need = frames * 2;
        if out.len() < need {
            return;
        }
        let start = self.last_started;
        out[..need].fill(0.0);

        let fade_step = 1.0 / (self.sample_rate as f32 * 0.005); // 5 ms ramp

        let mut i = 0;
        while i < self.voices.len() {
            let mut natural_end = false;
            {
                let v = &mut self.voices[i];
                if !v.playing && v.fade == 0.0 {
                    i += 1;
                    continue;
                }
                if v.stop_requested && v.fade_step == 0.0 {
                    v.fade_step = -fade_step;
                }
                // When paused (not stop), do not advance the read position.
                let advance = v.playing || v.stop_requested;
                let mut first_frame = 0usize;
                if v.delay_frames > 0 && advance {
                    let skip = (frames as u64).min(v.delay_frames) as usize;
                    v.delay_frames -= skip as u64;
                    first_frame = skip;
                }

                for f in first_frame..frames {
                    if v.pos >= v.end_frame {
                        if v.looping && v.end_frame > v.start_frame {
                            v.pos = v.start_frame;
                        } else if v.stop_requested || v.fade <= 0.0 {
                            natural_end = true;
                            break;
                        } else {
                            natural_end = true;
                            break;
                        }
                    }
                    if advance {
                        let (l, r) = cubic_sample(&v.data.samples, v.data.frames(), v.pos);
                        let g = v.gain * v.fade * self.master;
                        out[f * 2] += l * g;
                        out[f * 2 + 1] += r * g;
                        v.pos += v.step;
                    }
                    if v.fade_step != 0.0 {
                        v.fade = (v.fade + v.fade_step).clamp(0.0, 1.0);
                        if v.fade <= 0.0 {
                            natural_end = true;
                            break;
                        }
                    }
                }
            }
            if natural_end {
                let id = self.voices.swap_remove(i).id;
                self.push_finished(id);
                if start == Some(id) {
                    self.last_started = self.voices.last().map(|v| v.id);
                }
            } else {
                i += 1;
            }
        }

        // Final safety limiter (soft knee above -0.5 dBFS).
        for s in &mut out[..need] {
            let a = s.abs();
            if a > 0.94 {
                *s = (s.signum() * (0.94 + (a - 0.94) / (1.0 + (a - 0.94) * 4.0))).clamp(-1.0, 1.0);
            }
        }
    }
}

/// Catmull-Rom interpolation over stereo interleaved frames, clamped at edges.
#[inline]
fn cubic_sample(data: &[f32], frames: usize, pos: f64) -> (f32, f32) {
    if frames == 0 {
        return (0.0, 0.0);
    }
    let i = pos.floor() as isize;
    let frac = (pos - i as f64) as f32;
    let get = |idx: isize| -> (f32, f32) {
        let idx = idx.clamp(0, frames as isize - 1) as usize;
        (data[idx * 2], data[idx * 2 + 1])
    };
    let (l0, r0) = get(i - 1);
    let (l1, r1) = get(i);
    let (l2, r2) = get(i + 1);
    let (l3, r3) = get(i + 2);
    let interp = |p0: f32, p1: f32, p2: f32, p3: f32| {
        let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
        let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
        let c = -0.5 * p0 + 0.5 * p2;
        ((a * frac + b) * frac + c) * frac + p1
    };
    (interp(l0, l1, l2, l3), interp(r0, r1, r2, r3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: u32, ms: u64, freq: f32, amp: f32) -> Arc<DecodedSound> {
        let frames = (sr as u64 * ms / 1000) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for n in 0..frames {
            let s = (n as f32 / sr as f32 * freq * std::f32::consts::TAU).sin() * amp;
            samples.push(s);
            samples.push(s);
        }
        Arc::new(DecodedSound {
            sample_rate: sr,
            channels: 2,
            samples,
        })
    }

    #[test]
    fn mixes_two_voices_and_clamps() {
        let mut m = Mixer::new(48_000);
        m.play(sine(48_000, 100, 440.0, 0.8), 1.0, false, 0);
        m.play(sine(44_100, 100, 220.0, 0.8), 1.0, false, 0);
        let mut out = vec![0.0f32; 480 * 2];
        m.render(480, &mut out);
        assert!(out.iter().any(|s| s.abs() > 0.1));
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        assert_eq!(m.voice_count(), 2);
    }

    #[test]
    fn finishes_and_reports() {
        let mut m = Mixer::new(48_000);
        m.play(sine(48_000, 10, 440.0, 0.5), 1.0, false, 0);
        let mut out = vec![0.0f32; 480 * 2];
        for _ in 0..20 {
            m.render(480, &mut out);
        }
        assert_eq!(m.voice_count(), 0);
        assert_eq!(m.take_finished().len(), 1);
    }

    #[test]
    fn stop_fades_out() {
        let mut m = Mixer::new(48_000);
        let id = m.play(sine(48_000, 1000, 440.0, 0.5), 1.0, false, 0);
        let mut out = vec![0.0f32; 480 * 2];
        m.render(480, &mut out);
        m.stop(id);
        for _ in 0..10 {
            m.render(480, &mut out);
        }
        assert_eq!(m.voice_count(), 0);
    }

    #[test]
    fn delay_emits_silence_then_audio() {
        let mut m = Mixer::new(48_000);
        m.play_delayed(sine(48_000, 200, 440.0, 0.5), 1.0, false, 0, 50);
        let mut out = vec![0f32; 480 * 2]; // 10 ms
        m.render(480, &mut out);
        assert!(out.iter().all(|s| *s == 0.0), "first 10 ms must be silent");
        for _ in 0..5 {
            m.render(480, &mut out);
        }
        assert!(
            out.iter().any(|s| s.abs() > 0.01),
            "audio starts after 50 ms"
        );
    }

    #[test]
    fn pause_freezes_position() {
        let mut m = Mixer::new(48_000);
        let id = m.play(sine(48_000, 5000, 440.0, 0.5), 1.0, false, 0);
        let mut out = vec![0.0f32; 480 * 2];
        m.render(480, &mut out);
        let pos = m.position_ms(Some(id));
        m.pause_all();
        m.render(480, &mut out);
        assert_eq!(m.position_ms(Some(id)), pos);
        m.resume_all();
        m.render(480, &mut out);
        assert!(m.position_ms(Some(id)) > pos);
    }
}

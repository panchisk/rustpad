//! Sample-rate conversion.
//!
//! Two use cases:
//! 1. one-shot conversion of a whole buffer (mixer voices may instead interpolate
//!    on the fly, but editor/preview code uses this);
//! 2. streaming conversion for continuous microphone capture, where the resampler
//!    keeps its fractional position and filter state between calls.
//!
//! Quality: 4-point Catmull-Rom (cubic Hermite) interpolation. This is a cheap
//! interpolator with a good passband for gentle conversions (e.g. 44.1 <-> 48 kHz)
//! but it has no anti-aliasing filter. For large downsampling ratios the caller
//! should low-pass first; downsampling with this module is *stable* (no
//! feedback, no growing state) but not alias-free.

/// Number of source frames of history kept by [`StreamResampler`]. Catmull-Rom
/// needs the frame before the interpolated position, so four gives headroom.
const HISTORY_FRAMES: usize = 4;

/// Catmull-Rom interpolation between `y1` and `y2` with `t` in `[0, 1]`.
#[inline]
fn catmull_rom(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * t + b) * t + c) * t + y1
}

/// Interpolates a single channel of an interleaved buffer at frame position `x`.
#[inline]
fn channel_at(frames: &[f32], channels: usize, channel: usize, x: f64) -> f32 {
    let n = frames.len() / channels;
    if n == 0 {
        return 0.0;
    }
    let x = x.clamp(0.0, (n - 1) as f64);
    let i = x.floor() as isize;
    let t = (x - i as f64) as f32;
    let idx = |k: isize| k.clamp(0, n as isize - 1) as usize;
    let y0 = frames[idx(i - 1) * channels + channel];
    let y1 = frames[idx(i) * channels + channel];
    let y2 = frames[idx(i + 1) * channels + channel];
    let y3 = frames[idx(i + 2) * channels + channel];
    catmull_rom(y0, y1, y2, y3, t)
}

/// One-shot conversion of interleaved samples.
///
/// Output length is `round(input_frames * out_rate / in_rate)` frames. Edges are
/// replicated to avoid fade-in/fade-out artefacts.
pub fn resample_all(input: &[f32], in_rate: u32, out_rate: u32, channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if in_rate == 0 || out_rate == 0 || input.is_empty() {
        return Vec::new();
    }
    let in_frames = input.len() / ch;
    if in_frames == 0 {
        return Vec::new();
    }
    if in_rate == out_rate {
        return input[..in_frames * ch].to_vec();
    }

    let ratio = in_rate as f64 / out_rate as f64;
    let out_frames = ((in_frames as f64) * out_rate as f64 / in_rate as f64).round() as usize;
    let out_frames = out_frames.max(1);

    let mut out = Vec::with_capacity(out_frames * ch);
    for k in 0..out_frames {
        let x = k as f64 * ratio;
        for c in 0..ch {
            out.push(channel_at(input, ch, c, x));
        }
    }
    out
}

/// Stateful streaming resampler for interleaved audio.
///
/// Keeps the fractional read position and the last [`HISTORY_FRAMES`] source
/// frames so consecutive [`process`](Self::process) calls are seamless. Each call
/// consumes all of `input` and emits roughly `input_frames * out_rate / in_rate`
/// frames (integer), never reading past the end of the supplied block.
pub struct StreamResampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    /// Source frames consumed so far (start of the next block).
    consumed: u64,
    /// Absolute position (in source frames) of the next output frame.
    next_pos: f64,
    /// Last up to `HISTORY_FRAMES` input frames, interleaved.
    history: Vec<f32>,
    primed: bool,
}

impl StreamResampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: u16) -> Self {
        let mut rs = StreamResampler {
            in_rate,
            out_rate,
            channels: channels.max(1) as usize,
            consumed: 0,
            next_pos: 0.0,
            history: vec![0.0; HISTORY_FRAMES * channels.max(1) as usize],
            primed: false,
        };
        rs.reset();
        rs
    }

    /// Converts one interleaved block. Output length may differ from input;
    /// the object keeps fractional position and previous samples for continuity.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ch = self.channels;
        let n_in = input.len() / ch;
        if n_in == 0 {
            return Vec::new();
        }
        if self.in_rate == 0 || self.out_rate == 0 {
            return Vec::new();
        }
        if !self.primed {
            // Edge-replicate the first frame so the start of the stream matches
            // `resample_all` (which clamps indices) instead of fading from zero.
            let first: Vec<f32> = input[..ch].to_vec();
            self.history.clear();
            for _ in 0..HISTORY_FRAMES {
                self.history.extend_from_slice(&first);
            }
            self.primed = true;
        }
        if self.in_rate == self.out_rate {
            self.consumed += n_in as u64;
            self.next_pos += n_in as f64;
            return input[..n_in * ch].to_vec();
        }

        // Working buffer: history frames followed by this block.
        let hist_frames = self.history.len() / ch;
        let total_frames = hist_frames + n_in;
        let mut work = Vec::with_capacity(total_frames * ch);
        work.extend_from_slice(&self.history);
        work.extend_from_slice(&input[..n_in * ch]);

        let step = self.in_rate as f64 / self.out_rate as f64;
        // `rel` is the next output position in working-buffer coordinates.
        let start = self.next_pos - self.consumed as f64 + hist_frames as f64;
        let mut rel = start.max(0.0);

        let max_out = ((total_frames as f64) * self.out_rate as f64 / self.in_rate as f64).ceil()
            as usize
            + 4;
        let mut out = Vec::with_capacity(max_out * ch);

        loop {
            let i = rel.floor();
            if i < 1.0 {
                rel = 1.0;
                continue;
            }
            let i = i as usize;
            // Need frames i-1 ..= i+2 inside the working buffer.
            if i + 2 >= total_frames {
                break;
            }
            let t = (rel - i as f64) as f32;
            let base = i * ch;
            for c in 0..ch {
                let y0 = work[base - ch + c];
                let y1 = work[base + c];
                let y2 = work[base + ch + c];
                let y3 = work[base + 2 * ch + c];
                out.push(catmull_rom(y0, y1, y2, y3, t));
            }
            rel += step;
        }

        // Advance the absolute output position and keep the tail as history.
        self.next_pos = rel + self.consumed as f64 - hist_frames as f64;
        self.consumed += n_in as u64;

        let keep = total_frames.min(HISTORY_FRAMES);
        self.history.clear();
        self.history
            .extend_from_slice(&work[(total_frames - keep) * ch..]);

        out
    }

    pub fn reset(&mut self) {
        self.consumed = 0;
        self.next_pos = 0.0;
        self.history.clear();
        self.history.resize(HISTORY_FRAMES * self.channels, 0.0);
        self.primed = false;
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, ms: u32, freq: f32, amp: f32) -> Vec<f32> {
        let frames = (rate as u64 * ms as u64 / 1000) as usize;
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn upsample_length_close() {
        let input = sine(44_100, 500, 440.0, 0.5);
        let out = resample_all(&input, 44_100, 48_000, 1);
        let expected = input.len() as f64 * 48_000.0 / 44_100.0;
        assert!(
            (out.len() as f64 - expected).abs() <= 2.0,
            "len {} vs expected {}",
            out.len(),
            expected
        );
    }

    #[test]
    fn downsample_length_close() {
        let input = sine(48_000, 500, 440.0, 0.5);
        let out = resample_all(&input, 48_000, 44_100, 1);
        let expected = input.len() as f64 * 44_100.0 / 48_000.0;
        assert!((out.len() as f64 - expected).abs() <= 2.0);
    }

    #[test]
    fn dc_stays_dc() {
        let input = vec![0.5f32; 44_100];
        let out = resample_all(&input, 44_100, 48_000, 1);
        assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-3));

        let mut rs = StreamResampler::new(44_100, 48_000, 1);
        let streamed: Vec<f32> = rs.process(&input);
        assert!(streamed.iter().all(|s| (s - 0.5).abs() < 1e-3));
    }

    #[test]
    fn stereo_and_identity() {
        let interleaved: Vec<f32> = (0..1000)
            .flat_map(|i| {
                let v = i as f32 / 1000.0;
                [v, -v]
            })
            .collect();
        let same = resample_all(&interleaved, 48_000, 48_000, 2);
        assert_eq!(same, interleaved);

        let out = resample_all(&interleaved, 48_000, 44_100, 2);
        assert_eq!(out.len() % 2, 0);
        // Channels remain independent and symmetric.
        for f in out.chunks_exact(2) {
            assert!((f[0] + f[1]).abs() < 1e-4);
        }
    }

    #[test]
    fn streaming_two_blocks_matches_one_shot() {
        let input = sine(44_100, 200, 440.0, 0.7);
        let one_shot = resample_all(&input, 44_100, 48_000, 1);

        let mut rs = StreamResampler::new(44_100, 48_000, 1);
        let split = input.len() / 2;
        let mut streamed = rs.process(&input[..split]);
        streamed.extend(rs.process(&input[split..]));

        assert!(streamed.len() + 3 >= one_shot.len());
        // Streaming cannot reproduce the clamped tail of the one-shot pass, so
        // compare the interior.
        let n = streamed.len().min(one_shot.len()).saturating_sub(3);
        assert!(n > 0);
        for i in 0..n {
            assert!(
                (streamed[i] - one_shot[i]).abs() <= 1e-3,
                "mismatch at {i}: {} vs {}",
                streamed[i],
                one_shot[i]
            );
        }
    }

    #[test]
    fn streaming_is_continuous_across_blocks() {
        let input = sine(44_100, 300, 440.0, 0.8);
        let mut whole = StreamResampler::new(44_100, 48_000, 1);
        let reference = whole.process(&input);

        let mut split = StreamResampler::new(44_100, 48_000, 1);
        let mut streamed = Vec::new();
        for chunk in input.chunks(137) {
            streamed.extend(split.process(chunk));
        }
        assert!((streamed.len() as isize - reference.len() as isize).abs() <= 2);
        let n = streamed.len().min(reference.len());
        for i in 0..n {
            assert!((streamed[i] - reference[i]).abs() <= 2e-3, "at {i}");
        }
    }

    #[test]
    fn reset_restores_initial_state() {
        let input = sine(48_000, 100, 440.0, 0.5);
        let mut rs = StreamResampler::new(48_000, 44_100, 1);
        let first = rs.process(&input);
        rs.reset();
        let second = rs.process(&input);
        assert_eq!(first.len(), second.len());
        assert_eq!(first, second);
        assert_eq!(rs.output_rate(), 44_100);
    }

    #[test]
    fn empty_input_is_safe() {
        let mut rs = StreamResampler::new(44_100, 48_000, 2);
        assert!(rs.process(&[]).is_empty());
        assert!(resample_all(&[], 44_100, 48_000, 2).is_empty());
        assert!(resample_all(&[0.0; 4], 0, 48_000, 1).is_empty());
    }
}

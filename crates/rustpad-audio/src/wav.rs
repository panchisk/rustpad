//! WAV read/write helpers (hound).

use std::fs;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::types::DecodedSound;

/// Writes 16-bit PCM WAV (interleaved `f32` input, clamped).
pub fn write_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[f32]) -> Result<()> {
    if channels == 0 {
        return Err(anyhow!("cannot write WAV with zero channels"));
    }
    if sample_rate == 0 {
        return Err(anyhow!("cannot write WAV with zero sample rate"));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating directory {}", parent.display()))?;
        }
    }

    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating WAV file {}", path.display()))?;

    for &s in samples {
        let s = if s.is_finite() {
            s.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        // Scale by 32768 so that the round-trip through `read_wav` is exact for
        // signals representable in 16 bits.
        let v = (s * 32_768.0)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer
            .write_sample(v)
            .with_context(|| format!("writing WAV file {}", path.display()))?;
    }

    writer
        .finalize()
        .with_context(|| format!("finalising WAV file {}", path.display()))?;
    Ok(())
}

/// Reads a WAV file into interleaved `f32`.
pub fn read_wav(path: &Path) -> Result<DecodedSound> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("opening WAV file {}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels;
    if channels == 0 {
        return Err(anyhow!("WAV file {} has no channels", path.display()));
    }
    let sample_rate = spec.sample_rate;

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader
            .samples::<f32>()
            .map(|s| s.map(|v| if v.is_finite() { v } else { 0.0 }))
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("reading WAV samples from {}", path.display()))?,
        (hound::SampleFormat::Float, bits) => {
            return Err(anyhow!(
                "unsupported float WAV depth {bits} in {}",
                path.display()
            ))
        }
        (hound::SampleFormat::Int, 8) => read_int::<i8>(&mut reader, path, 128.0)?,
        (hound::SampleFormat::Int, 16) => read_int::<i16>(&mut reader, path, 32_768.0)?,
        (hound::SampleFormat::Int, 24) => read_int::<i32>(&mut reader, path, 8_388_608.0)?,
        (hound::SampleFormat::Int, 32) => read_int::<i32>(&mut reader, path, 2_147_483_648.0)?,
        (hound::SampleFormat::Int, bits) => {
            return Err(anyhow!(
                "unsupported integer WAV depth {bits} in {}",
                path.display()
            ))
        }
    };

    // Drop a trailing partial frame, if any.
    let ch = channels as usize;
    let usable = samples.len() - samples.len() % ch;
    let mut samples = samples;
    samples.truncate(usable);

    Ok(DecodedSound {
        sample_rate,
        channels,
        samples,
    })
}

fn read_int<T>(
    reader: &mut hound::WavReader<std::io::BufReader<std::fs::File>>,
    path: &Path,
    scale: f32,
) -> Result<Vec<f32>>
where
    T: hound::Sample + Into<i64>,
{
    reader
        .samples::<T>()
        .map(|s| {
            s.map(|v| {
                let v: i64 = v.into();
                (v as f32 / scale).clamp(-1.0, 1.0)
            })
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("reading WAV samples from {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_16_bit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rt.wav");
        let input: Vec<f32> = (0..2_000)
            .map(|i| ((i as f32 * 0.017).sin() * 0.8).clamp(-1.0, 1.0))
            .collect();
        write_wav(&path, 44_100, 2, &input).unwrap();
        let sound = read_wav(&path).unwrap();
        assert_eq!(sound.sample_rate, 44_100);
        assert_eq!(sound.channels, 2);
        assert_eq!(sound.samples.len(), input.len());
        for (a, b) in sound.samples.iter().zip(input.iter()) {
            assert!((a - b).abs() <= 1.0 / 32_768.0, "{a} vs {b}");
        }
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/out.wav");
        assert!(!path.parent().unwrap().exists());
        write_wav(&path, 8_000, 1, &[0.0, 0.25, -0.25]).unwrap();
        assert!(path.is_file());
        let sound = read_wav(&path).unwrap();
        assert_eq!(sound.frames(), 3);
    }

    #[test]
    fn read_8_24_and_32_bit() {
        let dir = tempfile::tempdir().unwrap();

        let spec8 = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 8,
            sample_format: hound::SampleFormat::Int,
        };
        let p8 = dir.path().join("u8.wav");
        let mut w = hound::WavWriter::create(&p8, spec8).unwrap();
        for v in [0i8, 64, -64, 127] {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let s8 = read_wav(&p8).unwrap();
        assert!((s8.samples[1] - 0.5).abs() < 1e-6);
        assert!((s8.samples[2] + 0.5).abs() < 1e-6);

        let spec24 = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        };
        let p24 = dir.path().join("s24.wav");
        let mut w = hound::WavWriter::create(&p24, spec24).unwrap();
        for v in [0i32, 4_194_304, -4_194_304] {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let s24 = read_wav(&p24).unwrap();
        assert!((s24.samples[1] - 0.5).abs() < 1e-6);
        assert!((s24.samples[2] + 0.5).abs() < 1e-6);

        let spec32 = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let p32 = dir.path().join("f32.wav");
        let mut w = hound::WavWriter::create(&p32, spec32).unwrap();
        for v in [0.0f32, 0.5, -0.25] {
            w.write_sample(v).unwrap();
        }
        w.finalize().unwrap();
        let s32 = read_wav(&p32).unwrap();
        assert_eq!(s32.samples, vec![0.0, 0.5, -0.25]);
    }

    #[test]
    fn clamps_and_rejects_bad_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clamp.wav");
        write_wav(&path, 8_000, 1, &[2.0, -2.0, f32::NAN]).unwrap();
        let s = read_wav(&path).unwrap();
        assert!(s.samples[0] > 0.99);
        assert!(s.samples[1] < -0.99);
        assert_eq!(s.samples[2], 0.0);

        assert!(write_wav(&path, 8_000, 0, &[0.0]).is_err());
        assert!(read_wav(&dir.path().join("missing.wav")).is_err());
    }
}

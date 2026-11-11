use std::path::Path;
use std::sync::Arc;

use leviathan_dsp::sampler::MAX_WAVE_LEN;

#[derive(Clone, Debug)]
pub struct DecodedWav {
    pub name: String,
    pub samples: Arc<[i16]>,
    pub c4_rate_hz: f32,
    pub original_frames: usize,
    pub original_sr: u32,
    pub peaks: [f32; 64],
}

pub fn load_wav_file(path: &Path) -> Result<DecodedWav, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Read error: {e}"))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("sample.wav")
        .to_string();
    decode_wav_bytes(&name, &bytes)
}
pub fn decode_wav_bytes(name: &str, bytes: &[u8]) -> Result<DecodedWav, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Not a valid RIFF/WAVE file".to_string());
    }

    let mut pos = 12usize;
    let mut fmt_tag: u16 = 0;
    let mut channels: usize = 0;
    let mut sample_rate: u32 = 0;
    let mut bits_per_sample: u16 = 0;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let data_start = pos + 8;
        let data_end = data_start.saturating_add(chunk_len).min(bytes.len());
        let chunk = &bytes[data_start..data_end];

        if chunk_id == b"fmt " && chunk.len() >= 16 {
            let raw_tag = u16::from_le_bytes([chunk[0], chunk[1]]);
            channels = u16::from_le_bytes([chunk[2], chunk[3]]) as usize;
            sample_rate = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
            bits_per_sample = u16::from_le_bytes([chunk[14], chunk[15]]);
            fmt_tag = if raw_tag == 0xFFFE && chunk.len() >= 26 {
                u16::from_le_bytes([chunk[24], chunk[25]])
            } else {
                raw_tag
            };
        } else if chunk_id == b"data" {
            data_slice = Some(chunk);
            if fmt_tag != 0 {
                break;
            }
        }

        let pad = chunk_len & 1;
        pos = data_end.saturating_add(pad);
    }

    if channels == 0 || sample_rate == 0 {
        return Err("Missing or invalid WAV fmt chunk".to_string());
    }
    let raw_data = data_slice.ok_or_else(|| "Missing WAV data chunk".to_string())?;
    let bytes_per_sample = (bits_per_sample as usize) / 8;
    if bytes_per_sample == 0 {
        return Err(format!("Unsupported bit depth: {bits_per_sample}"));
    }
    let frame_stride = bytes_per_sample * channels;
    let num_frames = raw_data.len() / frame_stride;
    if num_frames < 2 {
        return Err("WAV contains fewer than 2 sample frames".to_string());
    }

    let mut mono = Vec::with_capacity(num_frames);
    let ch_scale = 1.0 / channels as f32;

    for f in 0..num_frames {
        let base = f * frame_stride;
        let mut sum = 0.0_f32;
        for ch in 0..channels {
            let off = base + ch * bytes_per_sample;
            let s = match (fmt_tag, bits_per_sample) {
                (1, 8) => (raw_data[off] as f32 - 128.0) * (1.0 / 128.0),
                (1, 16) => {
                    i16::from_le_bytes([raw_data[off], raw_data[off + 1]]) as f32
                        * (1.0 / 32_768.0)
                }
                (1, 24) => {
                    let v = (raw_data[off] as i32)
                        | ((raw_data[off + 1] as i32) << 8)
                        | (((raw_data[off + 2] as i8) as i32) << 16);
                    v as f32 * (1.0 / 8_388_608.0)
                }
                (1, 32) => {
                    i32::from_le_bytes([
                        raw_data[off],
                        raw_data[off + 1],
                        raw_data[off + 2],
                        raw_data[off + 3],
                    ]) as f32
                        * (1.0 / 2_147_483_648.0)
                }
                (3, 32) => f32::from_le_bytes([
                    raw_data[off],
                    raw_data[off + 1],
                    raw_data[off + 2],
                    raw_data[off + 3],
                ]),
                (3, 64) => f64::from_le_bytes([
                    raw_data[off],
                    raw_data[off + 1],
                    raw_data[off + 2],
                    raw_data[off + 3],
                    raw_data[off + 4],
                    raw_data[off + 5],
                    raw_data[off + 6],
                    raw_data[off + 7],
                ]) as f32,
                _ => {
                    return Err(format!(
                        "Unsupported WAV encoding (tag={fmt_tag}, bits={bits_per_sample})"
                    ))
                }
            };
            if s.is_finite() {
                sum += s.clamp(-1.0, 1.0);
            }
        }
        mono.push((sum * ch_scale).clamp(-1.0, 1.0));
    }

    let (pcm_vec, c4_rate_hz) = if mono.len() <= MAX_WAVE_LEN {
        let pcm: Vec<i16> = mono
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
            .collect();
        (pcm, sample_rate as f32)
    } else {
        let mut pcm = Vec::with_capacity(MAX_WAVE_LEN);
        let ratio = mono.len() as f64 / MAX_WAVE_LEN as f64;
        for i in 0..MAX_WAVE_LEN {
            let s0 = (i as f64 * ratio) as usize;
            let s1 = (((i + 1) as f64 * ratio) as usize)
                .max(s0 + 1)
                .min(mono.len());
            let mut acc = 0.0_f32;
            for &v in &mono[s0..s1] {
                acc += v;
            }
            let avg = (acc / (s1 - s0) as f32).clamp(-1.0, 1.0);
            pcm.push((avg * 32_767.0) as i16);
        }
        let eff_sr = (sample_rate as f64 / ratio) as f32;
        (pcm, eff_sr)
    };

    let peaks = sample_peaks(&pcm_vec);
    Ok(DecodedWav {
        name: name.to_string(),
        samples: Arc::from(pcm_vec.into_boxed_slice()),
        c4_rate_hz,
        original_frames: num_frames,
        original_sr: sample_rate,
        peaks,
    })
}

pub fn sample_peaks(pcm: &[i16]) -> [f32; 64] {
    let mut peaks = [0.0_f32; 64];
    for (bin, slot) in peaks.iter_mut().enumerate() {
        let i0 = (bin * pcm.len()) / 64;
        let i1 = (((bin + 1) * pcm.len()) / 64)
            .max(i0 + 1)
            .min(pcm.len());
        let mut max_abs = 0.0_f32;
        for &s in &pcm[i0..i1] {
            let a = (s as f32 / 32_768.0).abs();
            if a > max_abs {
                max_abs = a;
            }
        }
        *slot = max_abs.clamp(0.0, 1.0);
    }

    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pcm16_wav(sr: u32, frames: &[i16]) -> Vec<u8> {
        let data_bytes = (frames.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + 8 + data_bytes).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"JUNK");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&sr.to_le_bytes());
        out.extend_from_slice(&(sr * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_bytes.to_le_bytes());
        for &s in frames {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn decodes_16bit_wav_and_preserves_short_rate() {
        let src: Vec<i16> = (0..1024)
            .map(|i| if i % 16 < 8 { 20_000 } else { -20_000 })
            .collect();
        let bytes = build_pcm16_wav(44_100, &src);
        let wav = decode_wav_bytes("kick.wav", &bytes).expect("valid wav");
        assert_eq!(wav.samples.len(), 1024);
        assert!((wav.c4_rate_hz - 44_100.0).abs() < 1.0);
        assert!(wav.peaks.iter().any(|&p| p > 0.5));
    }

    #[test]
    fn downsamples_long_wav_into_8192_frames_preserving_duration() {
        let src: Vec<i16> = (0..24_000)
            .map(|i| if i % 32 < 16 { 16_000 } else { -16_000 })
            .collect();
        let bytes = build_pcm16_wav(48_000, &src);
        let wav = decode_wav_bytes("loop.wav", &bytes).expect("valid wav");
        assert_eq!(wav.samples.len(), MAX_WAVE_LEN);
        let duration_s = wav.samples.len() as f32 / wav.c4_rate_hz;
        assert!((duration_s - 0.50).abs() < 0.01);
    }
}

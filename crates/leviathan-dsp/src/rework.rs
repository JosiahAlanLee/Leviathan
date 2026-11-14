//! Offline seeded grain scatter for mono and stereo PCM.

use crate::voice::sine_cycle;

pub const DEFAULT_SEED: u32 = 0x4153_5210;

#[derive(Clone, Copy)]
pub struct Params {
    pub seed: u32,
    pub smoothness: f32,
    pub amount: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            seed: DEFAULT_SEED,
            smoothness: 0.5,
            amount: 0.5,
        }
    }
}

/// Mutate a half-open PCM region. Each scratch entry is left, right, and weight.
/// Stereo shares grain positions and gain. Returns false for inconsistent buffers.
pub fn mutate<const CHANNELS: usize>(
    input: &[[i16; CHANNELS]],
    output: &mut [[i16; CHANNELS]],
    scratch: &mut [[f32; 3]],
    params: Params,
) -> bool {
    let frames = input.len();
    if !(1..=2).contains(&CHANNELS)
        || output.len() != frames
        || scratch.len() < frames
        || !params.amount.is_finite()
        || !params.smoothness.is_finite()
    {
        return false;
    }
    let amount = params.amount.clamp(0.0, 1.0);
    if amount == 0.0 || frames < 2 {
        output.copy_from_slice(input);
        return true;
    }

    let smoothness = params.smoothness.clamp(0.0, 1.0);
    // The tuning helper approximates 2^x. Here x is in its supported [-5, 2] range.
    let grain = (512.0 * crate::v_oct_to_hz(2.0 - 7.0 * smoothness) / 261.625_58 + 0.5) as usize;
    let grain = grain.clamp(2, frames);
    let hop = (grain / 4).max(1);
    let scratch = &mut scratch[..frames];
    scratch.fill([0.0; 3]);
    let mut seed = if params.seed == 0 {
        DEFAULT_SEED
    } else {
        params.seed
    };
    for target in (0..frames).step_by(hop) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let source = seed as usize % frames;
        for offset in 0..grain {
            let phase = (offset as f32 + 0.5) / grain as f32;
            let window = 0.5 - 0.5 * sine_cycle((phase + 0.25) % 1.0);
            let slot = &mut scratch[(target + offset) % frames];
            for (wet, dry) in slot.iter_mut().zip(input[(source + offset) % frames]) {
                *wet += f32::from(dry) * window;
            }
            slot[2] += window;
        }
    }
    for (wet, dry) in scratch.iter_mut().zip(input) {
        let weight = wet[2];
        for (wet, dry) in wet.iter_mut().zip(dry) {
            *wet = if weight > 0.0 {
                *wet / weight
            } else {
                f32::from(*dry)
            };
        }
    }
    let seam = ((8.0 + 120.0 * smoothness) as usize).min(frames / 4);
    for i in 0..seam {
        let weight = 0.5 * (1.0 - i as f32 / seam as f32);
        let end = frames - 1 - i;
        for ch in 0..CHANNELS {
            let difference = weight * (scratch[end][ch] - scratch[i][ch]);
            scratch[i][ch] += difference;
            scratch[end][ch] -= difference;
        }
    }

    let (mut dry_power, mut wet_power) = (0.0_f64, 0.0_f64);
    for (dry, wet) in input.iter().zip(scratch.iter()) {
        for ch in 0..CHANNELS {
            let dry = f64::from(dry[ch]);
            let wet = f64::from(wet[ch]);
            dry_power += dry * dry;
            wet_power += wet * wet;
        }
    }
    let power = if wet_power == 0.0 {
        1.0
    } else {
        (dry_power / wet_power).clamp(0.0625, 16.0)
    };
    // Newton iteration on a bounded positive ratio, independent of platform libm.
    let mut gain = 1.0;
    for _ in 0..8 {
        gain = (gain + power / gain) * 0.5;
    }
    let edge = (frames / 4).min(64);
    for (i, ((out, dry), wet)) in output.iter_mut().zip(input).zip(scratch).enumerate() {
        let distance = i.min(frames - 1 - i);
        if distance == 0 {
            *out = *dry;
            continue;
        }
        let mix = if distance < edge {
            let phase = 0.25 + (distance as f32 / edge as f32) * 0.5;
            amount * 0.5 * (1.0 - sine_cycle(phase))
        } else {
            amount
        };
        for ch in 0..CHANNELS {
            let dry = f32::from(dry[ch]);
            let sample = dry + mix * (wet[ch] * gain as f32 - dry);
            out[ch] = (sample + 0.5 * sample.signum()) as i16;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeatable_and_stereo_linked_even_at_slice_edges() {
        let a: [[i16; 2]; 277] = core::array::from_fn(|i| {
            let v = ((i * 751) % 21_817) as i16 - 10_000;
            [v, -v]
        });
        let mut b = [[0; 2]; 277];
        let mut c = b;
        let mut scratch = [[0.0; 3]; 277];
        assert!(mutate(&a, &mut b, &mut scratch, Params::default()));
        assert!(mutate(&a, &mut c, &mut scratch, Params::default()));
        assert_eq!(b, c);
        assert_ne!(a, b);
        assert_eq!(b[0], a[0]);
        assert_eq!(b[276], a[276]);
        assert!(b.iter().all(|[l, r]| i32::from(*l) + i32::from(*r) == 0));
    }

    #[test]
    fn short_slices_and_dry_mix_are_bounded() {
        let mut scratch = [[0.0; 3]; 2];
        let input = [[-32000], [1]];
        let mut out = [[0]; 2];
        assert!(mutate(&input, &mut out, &mut scratch, Params::default()));
        assert!(mutate(
            &input,
            &mut out,
            &mut scratch,
            Params {
                amount: 0.0,
                ..Params::default()
            }
        ));
        assert_eq!(out, input);
        assert!(!mutate(
            &input,
            &mut out,
            &mut scratch[..1],
            Params::default()
        ));
    }
}

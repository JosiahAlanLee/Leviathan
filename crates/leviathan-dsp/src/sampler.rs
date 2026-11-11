use leviathan_core::SoundLock;

use crate::{fast_sin_turns, lock_param};

pub const WAVE_LEN: usize = 1024;
pub const NUM_BANKS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleLoopMode {
    OneShot,
    ForwardLoop,
    PingPongLoop,
}

impl SampleLoopMode {
    #[inline]
    pub fn from_norm(v: f32) -> Self {
        if v < 0.33 {
            Self::OneShot
        } else if v < 0.66 {
            Self::ForwardLoop
        } else {
            Self::PingPongLoop
        }
    }
}

/// Shared 4-bank 16-bit PCM wave ROM/RAM (`8 KiB` total, `#![no_std]` safe).
#[derive(Clone, Debug)]
pub struct SampleBank {
    pub waves: [[i16; WAVE_LEN]; NUM_BANKS],
}

impl Default for SampleBank {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleBank {
    pub fn new() -> Self {
        let mut waves = [[0i16; WAVE_LEN]; NUM_BANKS];
        let mut rng: u32 = 0x517c_c1b7;

        for (i, slot) in waves[0].iter_mut().enumerate() {
            // Bank 0: Metallic industrial anvil strike (inharmonic partials + transient)
            let t = i as f32 / WAVE_LEN as f32;
            let env = 1.0 - 0.75 * t;
            let s = 0.50 * fast_sin_turns(t * 4.0)
                + 0.32 * fast_sin_turns(t * 10.928)
                + 0.18 * fast_sin_turns(t * 20.68);
            *slot = ((s * env).clamp(-1.0, 1.0) * 30_000.0) as i16;
        }

        for (i, slot) in waves[1].iter_mut().enumerate() {
            // Bank 1: Hollow 9-bit stepped PWM + vocal formant cycle
            let t = (i as f32 * 8.0) / WAVE_LEN as f32;
            let cyc = t - (t as i32 as f32);
            let pulse = if cyc < 0.32 { 0.75 } else { -0.45 };
            let formant = 0.35 * fast_sin_turns(t * 3.0) + 0.20 * fast_sin_turns(t * 5.0);
            let stepped = (((pulse + formant).clamp(-1.0, 1.0) * 128.0) as i32 as f32) / 128.0;
            *slot = (stepped * 29_000.0) as i16;
        }

        for (i, slot) in waves[2].iter_mut().enumerate() {
            // Bank 2: Minor-7th rave/dub chord cycle (root + m3 + 5th + m7)
            let t = (i as f32 * 8.0) / WAVE_LEN as f32;
            let chord = 0.34 * fast_sin_turns(t)
                + 0.24 * fast_sin_turns(t * 1.189_207)
                + 0.24 * fast_sin_turns(t * 1.498_307)
                + 0.18 * fast_sin_turns(t * 1.781_797);
            *slot = (chord.clamp(-1.0, 1.0) * 30_000.0) as i16;
        }

        for (i, slot) in waves[3].iter_mut().enumerate() {
            // Bank 3: Crunchy break/electro transient burst
            let t = i as f32 / WAVE_LEN as f32;
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let noise = (rng as i32 as f32) * (1.0 / 2_147_483_648.0);
            let body = fast_sin_turns(t * (14.0 - 6.0 * t)) * (1.0 - t);
            let mix = (0.62 * body + 0.38 * noise * (1.0 - 0.65 * t)).clamp(-1.0, 1.0);
            *slot = (mix * 30_500.0) as i16;
        }

        Self { waves }
    }

    pub fn load_pcm(&mut self, bank: usize, samples: &[i16]) {
        if bank >= NUM_BANKS || samples.is_empty() {
            return;
        }
        for (i, dst) in self.waves[bank].iter_mut().enumerate() {
            *dst = samples[i % samples.len()];
        }
    }
}

/// Clean-room UQ16.16 variable-clock transposing sampler voice.
///
/// SoundLock mapping (`0.0..=1.0`):
/// - `P0 (bit 0)`: Amplitude decay (`0.04s..1.10s`)
/// - `P1 (bit 1)`: DAC chip clock (`4.0 kHz..48.0 kHz`)
/// - `P2 (bit 2)`: Post-DAC SVF cutoff (handled by host rack)
/// - `P3 (bit 3)`: Wave bank select (`0..=3`)
/// - `P4 (bit 4)`: Loop start position (`0%..80%`)
/// - `P5 (bit 5)`: Loop window length (`32..1024` frames)
/// - `P6 (bit 6)`: Loop window scan rate (`0.0..1.0`)
/// - `P7 (bit 7)`: Loop mode (`OneShot` / `ForwardLoop` / `PingPongLoop`) & bit crush
#[derive(Clone, Debug)]
pub struct VarClockSamplerVoice {
    pos_q16: u32,
    dac_phase: f32,
    scan_phase: f32,
    reverse: bool,
    halted: bool,
    held_dac_sample: f32,
}

impl Default for VarClockSamplerVoice {
    fn default() -> Self {
        Self {
            pos_q16: 0,
            dac_phase: 0.0,
            scan_phase: 0.0,
            reverse: false,
            halted: false,
            held_dac_sample: 0.0,
        }
    }
}

impl VarClockSamplerVoice {
    pub fn trigger(&mut self, lock: &SoundLock) {
        let start_norm = lock_param(lock, 4, 0.0).clamp(0.0, 0.80);
        let start_idx = (start_norm * WAVE_LEN as f32) as u32;
        self.pos_q16 = start_idx << 16;
        self.dac_phase = 1.0;
        self.scan_phase = 0.0;
        self.reverse = false;
        self.halted = false;
    }

    pub fn step_sample(
        &mut self,
        bank: &SampleBank,
        pitch_hz: f32,
        host_sr: f32,
        lock: &SoundLock,
    ) -> f32 {
        if self.halted {
            return 0.0;
        }

        let dt = 1.0 / host_sr.max(8_000.0);
        let clock_norm = lock_param(lock, 1, 0.42).clamp(0.0, 1.0);
        let chip_clock_hz = 4_000.0 + 44_000.0 * clock_norm;

        let bank_idx = ((lock_param(lock, 3, 0.5).clamp(0.0, 0.999) * NUM_BANKS as f32) as usize)
            .min(NUM_BANKS - 1);
        let wave = &bank.waves[bank_idx];

        let scan_rate = lock_param(lock, 6, 0.15).clamp(0.0, 1.0);
        self.scan_phase = (self.scan_phase + scan_rate * 1.8 * dt) % 1.0;
        let scan_mod = 0.5 + 0.5 * fast_sin_turns(self.scan_phase);

        let base_start = lock_param(lock, 4, 0.0).clamp(0.0, 0.80);
        let win_norm = lock_param(lock, 5, 0.85).clamp(0.0, 1.0);
        let win_frames = 32 + ((win_norm * (WAVE_LEN as u32 - 32) as f32) as u32);
        let max_start = (WAVE_LEN as u32).saturating_sub(win_frames);
        let scanned_start = (((base_start + 0.35 * scan_rate * scan_mod).clamp(0.0, 1.0))
            * max_start as f32) as u32;
        let loop_start_q16 = scanned_start << 16;
        let loop_end_q16 = (scanned_start + win_frames).min(WAVE_LEN as u32) << 16;

        let mode_norm = lock_param(lock, 7, 0.55).clamp(0.0, 1.0);
        let loop_mode = SampleLoopMode::from_norm(mode_norm);

        // 8 base wave cycles span the 1024-sample table (128 samples per fundamental period)
        let samples_per_tick = (pitch_hz.clamp(20.0, 8_000.0) * 128.0) / chip_clock_hz;
        let step_q16 = ((samples_per_tick * 65_536.0) as u32).clamp(256, 8 << 16);

        self.dac_phase += chip_clock_hz * dt;
        while self.dac_phase >= 1.0 {
            self.dac_phase -= 1.0;

            if !self.reverse {
                let next = self.pos_q16.saturating_add(step_q16);
                if next >= loop_end_q16 {
                    match loop_mode {
                        SampleLoopMode::OneShot => {
                            self.halted = true;
                            self.held_dac_sample = 0.0;
                            return 0.0;
                        }
                        SampleLoopMode::ForwardLoop => {
                            let span = loop_end_q16.saturating_sub(loop_start_q16).max(1 << 16);
                            self.pos_q16 = loop_start_q16 + ((next - loop_start_q16) % span);
                        }
                        SampleLoopMode::PingPongLoop => {
                            self.reverse = true;
                            self.pos_q16 = loop_end_q16.saturating_sub(step_q16).max(loop_start_q16);
                        }
                    }
                } else {
                    self.pos_q16 = next;
                }
            } else if self.pos_q16 <= loop_start_q16.saturating_add(step_q16) {
                self.reverse = false;
                self.pos_q16 = loop_start_q16;
            } else {
                self.pos_q16 -= step_q16;
            }

            let idx0 = ((self.pos_q16 >> 16) as usize) % WAVE_LEN;
            let idx1 = (idx0 + 1) % WAVE_LEN;
            let frac = ((self.pos_q16 & 0xFFFF) as f32) * (1.0 / 65_536.0);
            let s0 = wave[idx0] as f32 * (1.0 / 32_768.0);
            let s1 = wave[idx1] as f32 * (1.0 / 32_768.0);

            // Blend raw zero-order DAC hold with linear interpolation as chip clock rises
            let interp = s0 + (s1 - s0) * (frac * clock_norm);
            let quant_levels = 64.0 + 1984.0 * clock_norm;
            self.held_dac_sample =
                (((interp * quant_levels) as i32 as f32) / quant_levels).clamp(-1.0, 1.0);
        }

        self.held_dac_sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_clock_and_ping_pong_loop_stay_bounded() {
        let bank = SampleBank::new();
        let mut voice = VarClockSamplerVoice::default();
        let lo_clock_lock = SoundLock {
            machine: 6,
            mask: (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7),
            values: [0.5, 0.05, 0.5, 0.6, 0.1, 0.25, 0.4, 0.85],
        };
        let hi_clock_lock = SoundLock {
            values: [0.5, 0.95, 0.5, 0.6, 0.1, 0.25, 0.4, 0.85],
            ..lo_clock_lock
        };

        voice.trigger(&lo_clock_lock);
        let mut lo_diff = 0.0_f32;
        let mut prev = 0.0_f32;
        for _ in 0..512 {
            let s = voice.step_sample(&bank, 220.0, 48_000.0, &lo_clock_lock);
            assert!(s.is_finite() && s.abs() <= 1.0);
            lo_diff += (s - prev).abs();
            prev = s;
        }

        voice.trigger(&hi_clock_lock);
        let mut hi_diff = 0.0_f32;
        prev = 0.0;
        for _ in 0..512 {
            let s = voice.step_sample(&bank, 220.0, 48_000.0, &hi_clock_lock);
            assert!(s.is_finite() && s.abs() <= 1.0);
            hi_diff += (s - prev).abs();
            prev = s;
        }

        assert!(lo_diff > 0.05 && hi_diff > 0.05);
        assert!((lo_diff - hi_diff).abs() > 0.01);
    }
}

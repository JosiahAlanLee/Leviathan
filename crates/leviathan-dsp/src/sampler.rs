use leviathan_core::SoundLock;

use crate::{fast_sin_turns, lock_param};

pub const ROM_WAVE_LEN: usize = 1024;
pub const MAX_WAVE_LEN: usize = 8192;
pub const NUM_BANKS: usize = 4;
pub const C4_HZ: f32 = 261.625_58;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleRegion {
    pub trim_start: f32,
    pub trim_end: f32,
    pub loop_enabled: bool,
}

impl Default for SampleRegion {
    fn default() -> Self {
        Self {
            trim_start: 0.0,
            trim_end: 1.0,
            loop_enabled: false,
        }
    }
}

impl SampleRegion {
    pub fn is_valid(self) -> bool {
        (0.0..=1.0).contains(&self.trim_start) && (0.0..=1.0).contains(&self.trim_end)
    }

    /// The half-open trim range always includes at least one source frame.
    pub fn frames(self, len: usize) -> core::ops::Range<usize> {
        let len = len.max(1);
        let start = ((self.trim_start.clamp(0.0, 1.0) * len as f32) as usize).min(len - 1);
        let end = ((self.trim_end.clamp(0.0, 1.0) * len as f32) as usize).clamp(start + 1, len);
        start..end
    }
}

#[derive(Clone, Debug)]
pub struct SampleBank {
    pub waves: [[i16; MAX_WAVE_LEN]; NUM_BANKS],
    pub lengths: [usize; NUM_BANKS],
    pub c4_rates: [f32; NUM_BANKS],
    pub regions: [SampleRegion; NUM_BANKS],
}

impl Default for SampleBank {
    fn default() -> Self {
        Self::new()
    }
}

impl SampleBank {
    pub fn new() -> Self {
        let mut bank = Self {
            waves: [[0i16; MAX_WAVE_LEN]; NUM_BANKS],
            lengths: [ROM_WAVE_LEN; NUM_BANKS],
            c4_rates: [C4_HZ * 128.0; NUM_BANKS],
            regions: [SampleRegion::default(); NUM_BANKS],
        };
        for b in 0..NUM_BANKS {
            bank.reset_rom_bank(b);
        }
        bank
    }

    pub fn reset_rom_bank(&mut self, bank_idx: usize) {
        if bank_idx >= NUM_BANKS {
            return;
        }
        self.lengths[bank_idx] = ROM_WAVE_LEN;
        self.c4_rates[bank_idx] = C4_HZ * 128.0;
        let wave = &mut self.waves[bank_idx];
        wave.fill(0);

        match bank_idx {
            0 => {
                for (i, slot) in wave[..ROM_WAVE_LEN].iter_mut().enumerate() {
                    let t = i as f32 / ROM_WAVE_LEN as f32;
                    let env = 1.0 - 0.75 * t;
                    let s = 0.50 * fast_sin_turns(t * 4.0)
                        + 0.32 * fast_sin_turns(t * 10.928)
                        + 0.18 * fast_sin_turns(t * 20.68);
                    *slot = ((s * env).clamp(-1.0, 1.0) * 30_000.0) as i16;
                }
            }
            1 => {
                for (i, slot) in wave[..ROM_WAVE_LEN].iter_mut().enumerate() {
                    let t = (i as f32 * 8.0) / ROM_WAVE_LEN as f32;
                    let cyc = t - (t as i32 as f32);
                    let pulse = if cyc < 0.32 { 0.75 } else { -0.45 };
                    let formant = 0.35 * fast_sin_turns(t * 3.0) + 0.20 * fast_sin_turns(t * 5.0);
                    let stepped =
                        (((pulse + formant).clamp(-1.0, 1.0) * 128.0) as i32 as f32) / 128.0;
                    *slot = (stepped * 29_000.0) as i16;
                }
            }
            2 => {
                for (i, slot) in wave[..ROM_WAVE_LEN].iter_mut().enumerate() {
                    let t = (i as f32 * 8.0) / ROM_WAVE_LEN as f32;
                    let chord = 0.34 * fast_sin_turns(t)
                        + 0.24 * fast_sin_turns(t * 1.189_207)
                        + 0.24 * fast_sin_turns(t * 1.498_307)
                        + 0.18 * fast_sin_turns(t * 1.781_797);
                    *slot = (chord.clamp(-1.0, 1.0) * 30_000.0) as i16;
                }
            }
            _ => {
                let mut rng: u32 = 0x517c_c1b7;
                for (i, slot) in wave[..ROM_WAVE_LEN].iter_mut().enumerate() {
                    let t = i as f32 / ROM_WAVE_LEN as f32;
                    rng ^= rng << 13;
                    rng ^= rng >> 17;
                    rng ^= rng << 5;
                    let noise = (rng as i32 as f32) * (1.0 / 2_147_483_648.0);
                    let body = fast_sin_turns(t * (14.0 - 6.0 * t)) * (1.0 - t);
                    let mix = (0.62 * body + 0.38 * noise * (1.0 - 0.65 * t)).clamp(-1.0, 1.0);
                    *slot = (mix * 30_500.0) as i16;
                }
            }
        }
    }

    pub fn load_pcm_with_rate(&mut self, bank: usize, samples: &[i16], c4_rate_hz: f32) {
        if bank >= NUM_BANKS || samples.is_empty() {
            return;
        }
        let len = samples.len().min(MAX_WAVE_LEN);
        self.waves[bank][..len].copy_from_slice(&samples[..len]);
        if len < MAX_WAVE_LEN {
            self.waves[bank][len..].fill(0);
        }
        self.lengths[bank] = len;
        self.c4_rates[bank] = c4_rate_hz.clamp(500.0, 192_000.0);
    }

    pub fn load_pcm(&mut self, bank: usize, samples: &[i16]) {
        self.load_pcm_with_rate(bank, samples, 48_000.0);
    }

    pub fn set_region(&mut self, bank: usize, region: SampleRegion) {
        if bank < NUM_BANKS && region.is_valid() {
            self.regions[bank] = region;
        }
    }
}

fn selected_bank(lock: &SoundLock) -> usize {
    (lock_param(lock, 3, 0.5).clamp(0.0, 0.999) * NUM_BANKS as f32) as usize
}

#[derive(Clone, Debug, Default)]
pub struct VarClockSamplerVoice {
    pos_q16: u32,
    dac_phase: f32,
    scan_phase: f32,
    reverse: bool,
    halted: bool,
    first_tick: bool,
    held_dac_sample: f32,
}

impl VarClockSamplerVoice {
    pub fn trigger(&mut self, bank: &SampleBank, lock: &SoundLock) {
        let bank_idx = selected_bank(lock);
        let active_len = bank.lengths[bank_idx].clamp(2, MAX_WAVE_LEN);
        let region = bank.regions[bank_idx];
        let trim = region.frames(active_len);
        let offset = if region.loop_enabled {
            0
        } else {
            (lock_param(lock, 4, 0.0).clamp(0.0, 0.80) * trim.len() as f32) as usize
        };
        self.pos_q16 = ((trim.start + offset) as u32) << 16;
        self.dac_phase = 1.0;
        self.scan_phase = 0.0;
        self.reverse = false;
        self.halted = false;
        self.first_tick = region != SampleRegion::default();
    }

    pub fn step_sample(
        &mut self,
        bank: &SampleBank,
        pitch_hz: f32,
        host_sr: f32,
        lock: &SoundLock,
    ) -> f32 {
        self.step_sample_gated(bank, pitch_hz, host_sr, lock, true)
    }

    pub fn step_sample_gated(
        &mut self,
        bank: &SampleBank,
        pitch_hz: f32,
        host_sr: f32,
        lock: &SoundLock,
        gate_high: bool,
    ) -> f32 {
        if self.halted {
            return 0.0;
        }

        let dt = 1.0 / host_sr.max(8_000.0);
        let clock_norm = lock_param(lock, 1, 0.42).clamp(0.0, 1.0);
        let chip_clock_hz = 4_000.0 + 44_000.0 * clock_norm;

        let bank_idx = selected_bank(lock);
        let active_len = bank.lengths[bank_idx].clamp(2, MAX_WAVE_LEN);
        let wave = &bank.waves[bank_idx][..active_len];
        let c4_rate = bank.c4_rates[bank_idx];
        let region = bank.regions[bank_idx];
        let trim = region.frames(active_len);
        let trim_start_q16 = (trim.start as u32) << 16;
        let trim_end_q16 = (trim.end as u32) << 16;
        if self.pos_q16 < trim_start_q16 || self.pos_q16 >= trim_end_q16 {
            self.pos_q16 = trim_start_q16;
            self.first_tick = true;
            self.reverse = false;
        }
        let (loop_start_q16, loop_end_q16, loop_mode) = if region.loop_enabled {
            self.reverse = false;
            let mode = if gate_high {
                SampleLoopMode::ForwardLoop
            } else {
                SampleLoopMode::OneShot
            };
            (trim_start_q16, trim_end_q16, mode)
        } else {
            let scan_rate = lock_param(lock, 6, 0.15).clamp(0.0, 1.0);
            self.scan_phase = (self.scan_phase + scan_rate * 1.8 * dt) % 1.0;
            let scan_mod = 0.5 + 0.5 * fast_sin_turns(self.scan_phase);
            let base_start = lock_param(lock, 4, 0.0).clamp(0.0, 0.80);
            let win_norm = lock_param(lock, 5, 0.85).clamp(0.0, 1.0);
            let min_win = 32.min(trim.len());
            let window = min_win + (win_norm * (trim.len() - min_win) as f32) as usize;
            let start = trim.start
                + ((base_start + 0.35 * scan_rate * scan_mod).clamp(0.0, 1.0)
                    * (trim.len() - window) as f32) as usize;
            let mode = SampleLoopMode::from_norm(lock_param(lock, 7, 0.55).clamp(0.0, 1.0));
            ((start as u32) << 16, ((start + window) as u32) << 16, mode)
        };

        let wave_frames_per_sec = c4_rate * (pitch_hz.clamp(20.0, 8_000.0) / C4_HZ);
        let samples_per_tick = wave_frames_per_sec / chip_clock_hz;
        let step_q16 = ((samples_per_tick * 65_536.0) as u32).clamp(64, 16 << 16);

        self.dac_phase += chip_clock_hz * dt;
        while self.dac_phase >= 1.0 {
            self.dac_phase -= 1.0;

            if core::mem::take(&mut self.first_tick) {
                // A newly trimmed voice begins on its first selected frame.
            } else if !self.reverse {
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
                            self.pos_q16 =
                                loop_end_q16.saturating_sub(step_q16).max(loop_start_q16);
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

            let idx0 = (self.pos_q16 >> 16) as usize;
            let idx1 = if idx0 + 1 < trim.end {
                idx0 + 1
            } else if region == SampleRegion::default() || (region.loop_enabled && gate_high) {
                trim.start
            } else {
                idx0
            };
            let frac = ((self.pos_q16 & 0xFFFF) as f32) * (1.0 / 65_536.0);
            let s0 = wave[idx0] as f32 * (1.0 / 32_768.0);
            let s1 = wave[idx1] as f32 * (1.0 / 32_768.0);

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
    fn trimmed_sustain_wraps_inside_the_region_and_finishes_after_gate_off() {
        let mut bank = SampleBank::new();
        bank.load_pcm(
            0,
            &[-32768, -32768, 4096, 8192, 12288, 16384, -32768, -32768],
        );
        let mut region = SampleRegion {
            trim_start: 0.25,
            trim_end: 0.75,
            loop_enabled: true,
        };
        bank.set_region(0, region);
        let lock = SoundLock {
            mask: (1 << 1) | (1 << 3) | (1 << 7),
            values: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            ..SoundLock::default()
        };
        let mut voice = VarClockSamplerVoice::default();
        voice.trigger(&bank, &lock);
        let mut wrapped = false;
        let mut last = voice.pos_q16;
        for _ in 0..32 {
            let sample = voice.step_sample_gated(&bank, C4_HZ, 96_000.0, &lock, true);
            assert!((0.125..=0.5).contains(&sample));
            assert!((2..6).contains(&(voice.pos_q16 >> 16)));
            wrapped |= voice.pos_q16 < last;
            last = voice.pos_q16;
        }
        assert!(wrapped && !voice.halted);
        for _ in 0..10 {
            let sample = voice.step_sample_gated(&bank, C4_HZ, 96_000.0, &lock, false);
            assert!((0.0..=0.5).contains(&sample));
            assert!(voice.pos_q16 >= last);
            last = voice.pos_q16;
        }
        assert!(voice.halted);

        region.trim_start = 1.0;
        region.trim_end = 0.25;
        assert_eq!(region.frames(8), 7..8);
        bank.set_region(0, region);
        voice.trigger(&bank, &lock);
        for _ in 0..32 {
            let sample = voice.step_sample_gated(&bank, 8000.0, 8000.0, &lock, true);
            assert_eq!(sample, -1.0);
            assert_eq!(voice.pos_q16 >> 16, 7);
        }
        region.trim_start = 0.25;
        assert_eq!(region.frames(8), 2..3);
    }

    #[test]
    fn variable_clock_and_custom_pcm_load_work() {
        let mut bank = SampleBank::new();
        let mut custom = [0i16; 4096];
        for (i, s) in custom.iter_mut().enumerate() {
            *s = if (i / 32) % 2 == 0 { 24_000 } else { -24_000 };
        }
        bank.load_pcm_with_rate(0, &custom, 24_000.0);
        assert_eq!(bank.lengths[0], 4096);

        let mut voice = VarClockSamplerVoice::default();
        let lock = SoundLock {
            machine: 4,
            mask: (1 << 1) | (1 << 3) | (1 << 5) | (1 << 7),
            values: [0.5, 0.25, 0.5, 0.0, 0.0, 1.0, 0.0, 0.50],
        };
        voice.trigger(&bank, &lock);
        let mut energy = 0.0_f32;
        for _ in 0..512 {
            let s = voice.step_sample(&bank, C4_HZ, 48_000.0, &lock);
            assert!(s.is_finite() && s.abs() <= 1.0);
            energy += s.abs();
        }
        assert!(energy > 10.0);
    }
}

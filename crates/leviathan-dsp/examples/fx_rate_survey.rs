//! Master effects across host rates: a Perc hit every 0.5 s for 3 s, then
//! 3 s of tail, through each effect at 44.1, 48 and 96 kHz. Writes stereo
//! interleaved f32 raw files for `fx_rate_survey.py`.
//!
//! `cargo run --release -p leviathan-dsp --example fx_rate_survey -- <out-dir>`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use leviathan_core::{CvFrame, NUM_LANES};
use leviathan_dsp::{
    echo, esp, BenderParams, CaptureSamples, DelayMode, LaneChannel, LanePatch, MachineKind,
    StereoDelay, SynthRack,
};

struct Fx {
    name: &'static str,
    channel: LaneChannel,
    delay: DelayMode,
    bender: BenderParams,
    send: f32,
    echo: Option<echo::Params>,
}

fn main() -> std::io::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "fx-rate".into());
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let dry = LaneChannel { level: 0.9, pan: 0.0, delay_send: 0.0, fx_send: 0.0 };
    let still = BenderParams { shift_norm: 0.5, feedback: 0.0, decay: 0.0, freeze: false };
    let fxs = [
        Fx { name: "dry", channel: dry, delay: DelayMode::Slapback, bender: still, send: 0.0, echo: None },
        Fx { name: "slapback", channel: LaneChannel { delay_send: 0.7, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0, echo: None },
        Fx { name: "pingpong", channel: LaneChannel { delay_send: 0.7, ..dry }, delay: DelayMode::LongPingPong, bender: still, send: 0.0, echo: None },
        Fx { name: "bender", channel: LaneChannel { fx_send: 0.8, ..dry }, delay: DelayMode::Slapback,
             bender: BenderParams { shift_norm: 0.6, feedback: 0.6, decay: 0.7, freeze: false }, send: 0.35, echo: None },
        Fx { name: "echo-free", channel: LaneChannel { fx_send: 0.8, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0,
             echo: Some(echo::Params { echo_mix: 0.5, time: 0.5, feedback: 0.55, ..Default::default() }) },
        Fx { name: "echo-sync", channel: LaneChannel { fx_send: 0.8, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0,
             echo: Some(echo::Params { echo_mix: 0.5, sync: echo::Sync::Transport, ratio: 4, feedback: 0.55, ..Default::default() }) },
        Fx { name: "echo-esp224", channel: LaneChannel { fx_send: 0.8, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0,
             echo: Some(echo::Params { echo_mix: 0.5, time: 0.4, feedback: 0.5, esp: esp::Mode::Rate224, ..Default::default() }) },
        Fx { name: "echo-shift", channel: LaneChannel { fx_send: 0.8, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0,
             echo: Some(echo::Params { echo_mix: 0.5, time: 0.3, feedback: 0.6, shift_hz: 30.0, ..Default::default() }) },
        Fx { name: "hot", channel: LaneChannel { level: 1.25, ..dry }, delay: DelayMode::Slapback, bender: still, send: 0.0, echo: None },
    ];
    for fx in &fxs {
        for rate in [44_100u32, 48_000, 96_000] {
            let r = rate as f32;
            let mut rack = SynthRack::new(r);
            let mut patch = LanePatch::for_machine(MachineKind::ElectroPerc);
            patch.params[4] = 0.0;
            rack.set_lane_patch(0, patch);
            let mut channels = [LaneChannel { level: 0.0, pan: 0.0, delay_send: 0.0, fx_send: 0.0 }; NUM_LANES];
            channels[0] = fx.channel;
            rack.set_lane_channels(channels);
            rack.set_delay_mode(fx.delay);
            rack.set_bender_params(fx.bender);
            rack.set_tempo(120.0);
            if let Some(e) = fx.echo {
                rack.set_echo_params(e);
            }
            rack.reset_voices();
            rack.reset_master(fx.send);
            let mut storage = if fx.echo.is_some() { vec![[0.0; 2]; echo::capacity(r)] } else { Vec::new() };
            let mut stems = [[0.0; 2]; NUM_LANES];
            let mut delays: Box<[StereoDelay; NUM_LANES]> = Box::new(std::array::from_fn(|_| {
                let mut d = StereoDelay::new(r);
                d.set_mode(fx.delay);
                d
            }));
            let samples = CaptureSamples::default();
            let mut cv = CvFrame::default();
            let half = (r * 0.5) as usize;
            let mut f = BufWriter::new(File::create(out.join(format!("{}-{rate}.f32", fx.name)))?);
            for n in 0..(r * 6.0) as usize {
                let hit = n < (r * 3.0) as usize && n % half < (r * 0.05) as usize;
                cv.gate_v[0] = if hit { 10.0 } else { 0.0 };
                cv.velocity_v[0] = 10.0;
                cv.run_v = 10.0;
                cv.clock_v = if (n * 48 / half) % 2 == 0 { 10.0 } else { 0.0 };
                let ((left, right), _) = rack.render_session_with_echo_stems(&cv, &samples, &mut stems, &mut delays, &mut storage);
                f.write_all(&left.to_le_bytes())?;
                f.write_all(&right.to_le_bytes())?;
            }
            f.flush()?;
        }
    }
    Ok(())
}

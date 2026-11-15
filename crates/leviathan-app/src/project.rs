use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use leviathan_core::rack_routing::RackRouting;
use leviathan_core::{
    Pattern, PatternChain, MAX_CHAIN_ENTRIES, MAX_NUDGE, MAX_RETRIG, MAX_SWING, NUM_LANES,
    ROWS_PER_PATTERN, SOUND_PARAMS,
};
use leviathan_dsp::sampler::{SampleBank, SampleRegion, MAX_WAVE_LEN, NUM_BANKS};
use leviathan_dsp::otis_kit::{
    FilterTaper, Lane as KitLane, Slot as KitSlot, VelocityCurve, MAX_FRAMES as MAX_KIT_FRAMES,
    SLOTS,
};
use leviathan_dsp::{default_lane_patches, echo, esp, BenderParams, DelayMode, LanePatch, MachineKind};
use serde::{Deserialize, Serialize};

use crate::capture::{Take, MAX_CAPTURE_FRAMES};
use crate::kit::{Kit, KitSample};
use crate::pcm;
use crate::lane_mix::LaneMix;
use crate::wav::{sample_peaks, DecodedWav};

pub const PATTERN_BANK_CAPACITY: usize = 16;
const PROJECT_VERSION: u32 = 2;
const MAX_PROJECT_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomShader {
    pub name: String,
    pub wgsl: String,
}
pub struct Project {
    pub bpm: f64,
    pub master_send: f32,
    pub delay_mode: DelayMode,
    pub bender_params: BenderParams,
    pub lane_mix: LaneMix,
    pub lane_patches: [LanePatch; NUM_LANES],
    pub patterns: Vec<Pattern>,
    pub selected_pattern: usize,
    pub chain: PatternChain,
    pub samples: [Option<DecodedWav>; NUM_BANKS],
    pub sample_regions: [SampleRegion; NUM_BANKS],
    pub custom_shader: Option<CustomShader>,
    pub voice_calibration: bool,
    pub instruments: InstrumentSettings,
}

#[derive(Clone, Default)]
pub struct InstrumentSettings {
    pub routing: RackRouting,
    pub take: Option<Arc<Take>>,
    pub kit: Kit,
    pub echo: echo::Params,
    pub note_gates: NoteGateSettings,
    pub bbus: BbusSettings,
    pub otis: OtisSettings,
    pub oversample: OversampleSettings,
}

/// FS77 and 4-Op FM at 4x with a band-limited Acid saw. Off (the default for
/// projects saved before it) keeps every lane at the original single rate;
/// on oversamples every lane not marked grit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OversampleSettings {
    pub enabled: bool,
    pub grit: [bool; NUM_LANES],
}

impl OversampleSettings {
    pub fn grit_lanes(self) -> u8 {
        if !self.enabled {
            return (1 << NUM_LANES) - 1;
        }
        self.grit
            .into_iter()
            .enumerate()
            .filter(|&(_, grit)| grit)
            .map(|(lane, _)| 1 << lane)
            .sum()
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// OTIS table set. Off (the default for projects saved before the warm
/// tables) maps TableSel over the four bright tables; on, each band's upper
/// half plays a warm twin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OtisSettings {
    pub warm_tables: bool,
}

impl OtisSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// BBUS register timing. Off (the default for projects saved before it
/// existed) keeps every lane free-running; on restarts the register each
/// note cycle, except on lanes marked grit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BbusSettings {
    pub cycle_lock: bool,
    pub grit: [bool; NUM_LANES],
}

impl BbusSettings {
    pub fn grit_lanes(self) -> u8 {
        if !self.cycle_lock {
            return (1 << NUM_LANES) - 1;
        }
        self.grit
            .into_iter()
            .enumerate()
            .filter(|&(_, grit)| grit)
            .map(|(lane, _)| 1 << lane)
            .sum()
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoteGateSettings {
    pub enabled: bool,
    pub one_shot: [bool; NUM_LANES],
}

impl NoteGateSettings {
    pub fn release_lanes(self) -> u8 {
        if !self.enabled {
            return 0;
        }
        self.one_shot
            .into_iter()
            .enumerate()
            .filter(|&(_, one_shot)| !one_shot)
            .map(|(lane, _)| 1 << lane)
            .sum()
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Project {
    pub fn sample_bank(&self) -> SampleBank {
        let mut bank = SampleBank::new();
        for (index, sample) in self.samples.iter().enumerate() {
            if let Some(sample) = sample {
                bank.load_pcm_with_rate(index, &sample.samples, sample.c4_rate_hz);
            }
            bank.set_region(index, self.sample_regions[index]);
        }
        bank
    }
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f64,
    #[serde(default)]
    master_send: f32,
    #[serde(default, with = "DelayModeDef")]
    delay_mode: DelayMode,
    #[serde(default, with = "BenderParamsDef")]
    bender_params: BenderParams,
    #[serde(flatten)]
    lane_mix: LaneMix,
    /// Per-lane instrument patches; absent in projects saved before `T15`.
    #[serde(default)]
    lane_patches: Option<[StoredLanePatch; NUM_LANES]>,
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: Vec<u8>,
    sample_banks: [Option<SampleFile>; NUM_BANKS],
    #[serde(default)]
    sample_regions: [StoredSampleRegion; NUM_BANKS],
    #[serde(default)]
    custom_shader: Option<CustomShader>,
    #[serde(default)]
    voice_calibration: bool,
    #[serde(default, skip_serializing_if = "InstrumentFile::is_default")]
    instruments: InstrumentFile,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct InstrumentFile {
    routing: RackRouting,
    take: Option<CaptureFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kit: Option<KitFile>,
    #[serde(default, with = "EchoParamsDef")]
    echo: echo::Params,
    #[serde(skip_serializing_if = "NoteGateSettings::is_default")]
    note_gates: NoteGateSettings,
    #[serde(skip_serializing_if = "BbusSettings::is_default")]
    bbus: BbusSettings,
    #[serde(skip_serializing_if = "OtisSettings::is_default")]
    otis: OtisSettings,
    #[serde(skip_serializing_if = "OversampleSettings::is_default")]
    oversample: OversampleSettings,
}

impl InstrumentFile {
    fn is_default(&self) -> bool {
        self.routing == RackRouting::default()
            && self.take.is_none()
            && self.kit.is_none()
            && self.echo == echo::Params::default()
            && self.note_gates.is_default()
            && self.bbus.is_default()
            && self.otis.is_default()
            && self.oversample.is_default()
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum CaptureFile {
    Compact { rate: u32, frame_count: usize, pcm_s16le: String },
    Legacy { rate: u32, frames: Vec<[i16; 2]> },
}

impl<'de> Deserialize<'de> for CaptureFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            rate: u32,
            frame_count: Option<usize>,
            pcm_s16le: Option<String>,
            frames: Option<LegacyPcm>,
        }

        let stored = Stored::deserialize(deserializer)?;
        match (stored.frame_count, stored.pcm_s16le, stored.frames) {
            (Some(frame_count), Some(pcm_s16le), None) => Ok(Self::Compact {
                rate: stored.rate,
                frame_count,
                pcm_s16le,
            }),
            (None, None, Some(LegacyPcm(frames))) => Ok(Self::Legacy {
                rate: stored.rate,
                frames,
            }),
            _ => Err(serde::de::Error::custom("Invalid capture PCM fields")),
        }
    }
}

struct LegacyPcm(Vec<[i16; 2]>);

impl<'de> Deserialize<'de> for LegacyPcm {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Frames;
        impl<'de> serde::de::Visitor<'de> for Frames {
            type Value = LegacyPcm;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("stereo PCM within the capture frame budget")
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let limit_error = || serde::de::Error::custom("Capture exceeds its frame budget");
                let known_size = seq.size_hint().unwrap_or(0);
                if known_size > MAX_CAPTURE_FRAMES {
                    return Err(limit_error());
                }
                let mut frames = Vec::with_capacity(known_size);
                while let Some(frame) = seq.next_element()? {
                    if frames.len() == MAX_CAPTURE_FRAMES {
                        return Err(limit_error());
                    }
                    if frames.len() == frames.capacity() {
                        let extra = frames.capacity().max(1024).min(MAX_CAPTURE_FRAMES - frames.len());
                        frames.reserve_exact(extra);
                    }
                    frames.push(frame);
                }
                Ok(LegacyPcm(frames))
            }
        }
        deserializer.deserialize_seq(Frames)
    }
}

impl CaptureFile {
    fn decode(self) -> Result<Arc<Take>, String> {
        let (rate, frames) = match self {
            Self::Compact { rate, frame_count, pcm_s16le } => {
                if rate == 0 { return Err("Invalid capture source rate".into()); }
                (rate, pcm::decode(&pcm_s16le, frame_count, MAX_CAPTURE_FRAMES)?)
            }
            Self::Legacy { rate, frames } => (rate, frames),
        };
        Take::new(rate, frames).ok_or_else(|| "Invalid capture sample".to_string())
    }
}

#[derive(Serialize, Deserialize)]
struct KitFile {
    samples: [Option<KitSampleFile>; SLOTS],
    params: [StoredSlot; SLOTS],
    lanes: [StoredKitLane; NUM_LANES],
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "KitSlot")]
struct KitSlotDef {
    tune: f32,
    decay: f32,
    level: f32,
    filter: f32,
    start: f32,
    reverse: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "KitLane")]
struct KitLaneDef {
    bytebus: bool,
    clock: u8,
    drop_samples: bool,
    // Projects saved before the soft curve existed load on the chip curve.
    #[serde(default, with = "VelocityCurveDef", skip_serializing_if = "is_chip_curve")]
    velocity: VelocityCurve,
    // Kits saved before reconstruction and the linear filter taper keep their sound.
    #[serde(default = "legacy_raw_output")]
    raw_output: bool,
    #[serde(default = "legacy_filter_taper", with = "FilterTaperDef")]
    filter_taper: FilterTaper,
}

fn legacy_raw_output() -> bool {
    true
}

fn legacy_filter_taper() -> FilterTaper {
    FilterTaper::Squared
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "FilterTaper")]
enum FilterTaperDef {
    Linear,
    Squared,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "VelocityCurve")]
enum VelocityCurveDef {
    Chip,
    Soft,
}

fn is_chip_curve(curve: &VelocityCurve) -> bool {
    *curve == VelocityCurve::Chip
}

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct StoredSlot(#[serde(with = "KitSlotDef")] KitSlot);

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct StoredKitLane(#[serde(with = "KitLaneDef")] KitLane);

#[derive(Serialize, Deserialize)]
struct KitSampleFile {
    name: String,
    rate: u32,
    frame_count: usize,
    pcm_s16le: String,
}

impl KitFile {
    fn from_kit(kit: &Kit) -> Option<Self> {
        if kit.samples.iter().all(Option::is_none)
            && kit.params == [KitSlot::default(); SLOTS] && kit.lanes == [KitLane::default(); NUM_LANES] {
            return None;
        }
        Some(Self {
            samples: kit.samples.each_ref().map(|slot| slot.as_ref().map(|s| KitSampleFile {
                name: s.name.clone(),
                rate: s.rate,
                frame_count: s.pcm.len(),
                pcm_s16le: pcm::encode(s.pcm.as_chunks::<1>().0),
            })),
            params: kit.params.map(StoredSlot),
            lanes: kit.lanes.map(StoredKitLane),
        })
    }

    fn decode(self) -> Result<Kit, String> {
        let mut kit = Kit { params: self.params.map(|v| v.0), lanes: self.lanes.map(|v| v.0), ..Kit::default() };
        let mut count = 0usize;
        for slot in self.samples.iter().flatten() {
            count = count.saturating_add(slot.frame_count);
            if slot.name.is_empty() || slot.name.len() > 256 || !(1_000..=768_000).contains(&slot.rate)
                || slot.frame_count < 2 || count > MAX_KIT_FRAMES {
                return Err("OtisKit sample name, rate or frame budget is invalid".into());
            }
        }
        if !kit.is_valid() {
            return Err("Invalid OtisKit controls".into());
        }
        for (dest, file) in kit.samples.iter_mut().zip(self.samples) {
            if let Some(file) = file {
                let frames = pcm::decode::<1>(&file.pcm_s16le, file.frame_count, MAX_KIT_FRAMES)?;
                let pcm = frames.into_iter().map(|[v]| v).collect();
                *dest = Some(KitSample::new(file.name, file.rate, pcm)?);
            }
        }
        Ok(kit)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "echo::Params")]
struct EchoParamsDef {
    echo_mix: f32,
    space_mix: f32,
    time: f32,
    #[serde(with = "EchoSyncDef")]
    sync: echo::Sync,
    ratio: u8,
    feedback: f32,
    tone: f32,
    spread: f32,
    shift_hz: f32,
    shift_in_loop: bool,
    #[serde(with = "EspModeDef")]
    esp: esp::Mode,
    limit_64k: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "echo::Sync", rename_all = "snake_case")]
enum EchoSyncDef { Free, Transport, ByteBus }

#[derive(Serialize, Deserialize)]
#[serde(remote = "esp::Mode", rename_all = "snake_case")]
enum EspModeDef { Off, Rate336, Rate224, Rate128 }

#[derive(Serialize, Deserialize)]
#[serde(remote = "DelayMode", rename_all = "snake_case")]
enum DelayModeDef {
    Slapback,
    LongPingPong,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "BenderParams")]
struct BenderParamsDef {
    shift_norm: f32,
    feedback: f32,
    decay: f32,
    freeze: bool,
}

/// On-disk form of a `LanePatch`: the machine is stored by its stable slug.
#[derive(Serialize, Deserialize)]
struct StoredLanePatch {
    machine: String,
    params: [f32; SOUND_PARAMS],
}

impl From<&LanePatch> for StoredLanePatch {
    fn from(patch: &LanePatch) -> Self {
        Self {
            machine: patch.machine.slug().to_string(),
            params: patch.params,
        }
    }
}

impl StoredLanePatch {
    fn decode(&self) -> Result<LanePatch, String> {
        let machine = MachineKind::from_slug(&self.machine)
            .ok_or_else(|| format!("Unknown lane machine '{}'", self.machine))?;
        let patch = LanePatch {
            machine,
            params: self.params,
        };
        if !patch.is_valid() {
            return Err("Lane patch parameters must be between 0 and 1".to_string());
        }
        Ok(patch)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "SampleRegion")]
struct SampleRegionDef {
    trim_start: f32,
    trim_end: f32,
    loop_enabled: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
struct StoredSampleRegion(#[serde(with = "SampleRegionDef")] SampleRegion);

#[derive(Serialize, Deserialize)]
struct SampleFile {
    name: String,
    pcm: Vec<i16>,
    c4_rate_hz: f32,
    original_frames: usize,
    original_sr: u32,
}

impl From<&DecodedWav> for SampleFile {
    fn from(sample: &DecodedWav) -> Self {
        Self {
            name: sample.name.clone(),
            pcm: sample.samples.to_vec(),
            c4_rate_hz: sample.c4_rate_hz,
            original_frames: sample.original_frames,
            original_sr: sample.original_sr,
        }
    }
}

impl SampleFile {
    fn decode(self) -> Result<DecodedWav, String> {
        if !(2..=MAX_WAVE_LEN).contains(&self.pcm.len())
            || self.original_frames < self.pcm.len()
            || self.original_sr == 0
            || !self.c4_rate_hz.is_finite()
            || self.c4_rate_hz <= 0.0
        {
            return Err("Invalid sample bank data".to_string());
        }
        let peaks = sample_peaks(&self.pcm);
        Ok(DecodedWav {
            name: self.name,
            samples: Arc::from(self.pcm),
            c4_rate_hz: self.c4_rate_hz,
            original_frames: self.original_frames,
            original_sr: self.original_sr,
            peaks,
        })
    }
}

impl ProjectFile {
    fn into_project(self) -> Result<Project, String> {
        if !matches!(self.version, 1 | PROJECT_VERSION) {
            return Err(format!("Unsupported project version {}", self.version));
        }
        if !(40.0..=240.0).contains(&self.bpm) {
            return Err("BPM must be between 40 and 240".to_string());
        }
        if !(0.0..=1.0).contains(&self.master_send) {
            return Err("Master delay send must be between 0 and 1".to_string());
        }
        if !self.bender_params.is_valid() {
            return Err("BEND shift, feedback and decay must be between 0 and 1".to_string());
        }
        if !self.lane_mix.is_valid() {
            return Err("Invalid lane mute or solo mask or mixer settings".to_string());
        }
        let routing = self.instruments.routing;
        if routing.render_order().is_none() {
            return Err("Invalid or cyclic instrument routing".to_string());
        }
        let echo = self.instruments.echo;
        if !echo.is_valid() { return Err("Invalid BEND Echo or ESP settings".into()); }
        let kit = self.instruments.kit.map(KitFile::decode).transpose()?.unwrap_or_default();
        let take = self.instruments.take.map(CaptureFile::decode).transpose()?;
        let mut lane_patches = default_lane_patches();
        if let Some(stored) = &self.lane_patches {
            for (patch, stored) in lane_patches.iter_mut().zip(stored) {
                *patch = stored.decode()?;
            }
        }
        if !(1..=PATTERN_BANK_CAPACITY).contains(&self.patterns.len())
            || self.selected_pattern >= self.patterns.len()
        {
            return Err("Invalid pattern bank or selection".to_string());
        }
        if self.chain.len() > MAX_CHAIN_ENTRIES
            || self
                .chain
                .iter()
                .any(|&id| usize::from(id) >= self.patterns.len())
        {
            return Err("Invalid pattern chain".to_string());
        }
        for (id, pattern) in self.patterns.iter().enumerate() {
            validate_pattern(pattern).map_err(|error| format!("Pattern {id:02X}: {error}"))?;
        }
        let mut chain = PatternChain::default();
        for id in self.chain {
            chain.push(id);
        }
        let mut samples = core::array::from_fn(|_| None);
        let sample_regions = self.sample_regions.map(|region| region.0);
        for (bank, region) in sample_regions.iter().enumerate() {
            if !region.is_valid() {
                return Err(format!("Bank B{bank}: invalid sample trim"));
            }
        }
        for (bank, sample) in self.sample_banks.into_iter().enumerate() {
            samples[bank] = sample
                .map(SampleFile::decode)
                .transpose()
                .map_err(|error| format!("Bank B{bank}: {error}"))?;
        }
        if let Some(ref shader) = self.custom_shader {
            if shader.name.trim().is_empty() || shader.name.len() > 256 {
                return Err("Invalid custom shader name".to_string());
            }
            leviathan_gpu::validate_wgsl_contract(&shader.wgsl)?;
        }
        Ok(Project {
            bpm: self.bpm,
            master_send: self.master_send,
            delay_mode: self.delay_mode,
            bender_params: self.bender_params,
            lane_mix: self.lane_mix,
            lane_patches,
            patterns: self.patterns,
            selected_pattern: self.selected_pattern,
            chain,
            samples,
            sample_regions,
            custom_shader: self.custom_shader,
            voice_calibration: self.voice_calibration,
            instruments: InstrumentSettings {
                routing,
                take,
                kit,
                echo,
                note_gates: self.instruments.note_gates,
                bbus: self.instruments.bbus,
                otis: self.instruments.otis,
                oversample: self.instruments.oversample,
            },
        })
    }
}

fn validate_pattern(pattern: &Pattern) -> Result<(), &'static str> {
    if !(1..=ROWS_PER_PATTERN).contains(&pattern.length) {
        return Err("Invalid pattern length");
    }
    if !(0.5..=MAX_SWING).contains(&pattern.swing) {
        return Err("Pattern swing must be between 50% and 75%");
    }
    for cell in pattern.lanes.iter().flatten() {
        if cell.midi > 127
            || cell.velocity > 127
            || cell.probability > 100
            || !(0.0..=1.0).contains(&cell.gate_fraction)
        {
            return Err("Invalid note cell");
        }
        if !(-MAX_NUDGE..=MAX_NUDGE).contains(&cell.micro_offset)
            || !(1..=MAX_RETRIG).contains(&cell.retrig)
        {
            return Err("Invalid step nudge or retrigger count");
        }
        if !MachineKind::is_step_lock_valid(cell.lock.machine)
            || cell.lock.mask > 0xff
            || cell.lock.values.iter().any(|value| !value.is_finite())
        {
            return Err("Invalid sound lock");
        }
    }
    for row in &pattern.automation {
        if !(0.0..=1.0).contains(&row.z)
            || !(-1.0..=1.0).contains(&row.roll)
            || !(0.0..=1.0).contains(&row.feedback)
        {
            return Err("Invalid visual automation");
        }
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Project, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_PROJECT_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PROJECT_BYTES {
        return Err("Project exceeds 8 MiB".to_string());
    }
    let file: ProjectFile = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    file.into_project()
}

pub fn save(path: &Path, project: Project) -> Result<(), String> {
    if project.instruments.routing.render_order().is_none() {
        return Err("Invalid or cyclic instrument routing".to_string());
    }
    if !project.instruments.kit.is_valid() || !project.instruments.echo.is_valid() {
        return Err("Invalid OtisKit or BEND Echo settings".into());
    }
    let project = ProjectFile {
        version: PROJECT_VERSION,
        bpm: project.bpm,
        master_send: project.master_send,
        delay_mode: project.delay_mode,
        bender_params: project.bender_params,
        lane_mix: project.lane_mix,
        lane_patches: Some(core::array::from_fn(|lane| {
            StoredLanePatch::from(&project.lane_patches[lane])
        })),
        patterns: project.patterns,
        selected_pattern: project.selected_pattern,
        chain: project.chain.as_slice().to_vec(),
        sample_banks: core::array::from_fn(|bank| {
            project.samples[bank].as_ref().map(SampleFile::from)
        }),
        sample_regions: project.sample_regions.map(StoredSampleRegion),
        custom_shader: project.custom_shader,
        voice_calibration: project.voice_calibration,
        instruments: InstrumentFile {
            routing: project.instruments.routing,
            take: project.instruments.take.as_deref().map(|take| CaptureFile::Compact {
                rate: take.rate,
                frame_count: take.frames.len(),
                pcm_s16le: pcm::encode(&take.frames),
            }),
            kit: KitFile::from_kit(&project.instruments.kit),
            echo: project.instruments.echo,
            note_gates: project.instruments.note_gates,
            bbus: project.instruments.bbus,
            otis: project.instruments.otis,
            oversample: project.instruments.oversample,
        },
    };
    let mut bytes = serde_json::to_vec(&project).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_PROJECT_BYTES {
        return Err("Project exceeds 8 MiB".to_string());
    }
    atomic_write(path, &bytes).map_err(|error| error.to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let (temporary, mut file) = loop {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".leviathan-{}-{id}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let written = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    let result = written.and_then(|_| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use leviathan_core::{AutomationRow, FillCondition, NoteCell, PlaybackRate, SoundLock};

    #[test]
    fn project_replaces_saved_file_and_round_trips_patterns_and_embedded_samples() {
        let path =
            std::env::temp_dir().join(format!("leviathan-round-trip-{}.json", std::process::id()));
        let mut patterns: Vec<_> = std::iter::repeat_with(Pattern::default).take(2).collect();
        patterns[0].lanes[1][0].lock.machine = MachineKind::OtisTwin as i16;
        patterns[1].rate = PlaybackRate::Half;
        patterns[1].swing = 0.625;
        patterns[1].lanes[4][63] = NoteCell {
            active: true,
            air: true,
            fill: FillCondition::WhenNoFill,
            probability: 37,
            micro_offset: -9,
            retrig: 3,
            lock: SoundLock {
                machine: 4,
                mask: 0x11,
                values: [0.123_456_79; 8],
            },
            ..NoteCell::default()
        };
        patterns[1].automation[0] = AutomationRow {
            z: 0.75,
            roll: -0.0,
            feedback: 0.42,
            byte_word: 0x98,
            byte_override: false,
        };
        patterns[1].automation[1].byte_override = true;
        let mut chain = PatternChain::default();
        for id in [0, 1, 1] {
            assert!(chain.push(id));
        }
        let pcm = Arc::<[i16]>::from([0, 1000, -2000, 3000]);
        let mut samples = core::array::from_fn(|_| None);
        samples[2] = Some(DecodedWav {
            name: "memory-only.wav".to_string(),
            c4_rate_hz: 22_049.998,
            original_frames: 4,
            original_sr: 22_050,
            peaks: sample_peaks(&pcm),
            samples: Arc::clone(&pcm),
        });
        let mut sample_regions = [SampleRegion::default(); NUM_BANKS];
        sample_regions[0].trim_end = 0.125;
        sample_regions[2] = SampleRegion {
            trim_start: 0.25,
            trim_end: 0.75,
            loop_enabled: true,
        };
        let bender_params = BenderParams {
            shift_norm: 0.16,
            feedback: 0.8,
            decay: 0.7,
            freeze: true,
        };
        let mut lane_patches = default_lane_patches();
        lane_patches[0] = LanePatch::for_machine(MachineKind::OtisTwin);
        let snapshot = |bpm, master_send, selected_pattern, lane_mix| Project {
            bpm,
            master_send,
            delay_mode: DelayMode::LongPingPong,
            bender_params,
            lane_patches,
            lane_mix,
            selected_pattern,
            chain,
            patterns: patterns.clone(),
            samples: samples.clone(),
            sample_regions,
            custom_shader: None,
            voice_calibration: true,
            instruments: Default::default(),
        };
        save(&path, snapshot(120.0, 0.0, 0, LaneMix::default())).unwrap();
        let bpm = f64::from_bits(120.0_f64.to_bits() + 1);
        let send = 0.37_f32;
        let mut mix = LaneMix {
            mute_mask: 0b10,
            solo_mask: 0b1010,
            ..LaneMix::default()
        };
        mix.channels[4].0.level = 1.25;
        mix.channels[4].0.pan = -0.5;
        mix.channels[4].0.delay_send = 0.75;
        mix.channels[4].0.fx_send = 0.61;
        save(&path, snapshot(bpm, send, 1, mix)).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(stored.contains(r#""machine":"otis2""#));
        assert!(stored.contains(r#""machine":14"#));
        let project = load(&path).unwrap();
        let mut legacy_json: serde_json::Value = serde_json::from_str(&stored).unwrap();
        legacy_json.as_object_mut().unwrap().remove("voice_calibration");
        fs::write(&path, serde_json::to_vec(&legacy_json).unwrap()).unwrap();
        let legacy = load(&path).unwrap();
        assert!(!legacy.voice_calibration);
        assert_eq!(legacy.lane_patches, lane_patches);
        fs::remove_file(path).unwrap();
        assert_eq!(project.bpm.to_bits(), bpm.to_bits());
        assert_eq!(project.master_send.to_bits(), send.to_bits());
        assert_eq!(project.delay_mode, DelayMode::LongPingPong);
        assert_eq!(project.bender_params, bender_params);
        assert!(project.voice_calibration);
        assert_eq!(project.lane_patches, lane_patches);
        assert_eq!(project.lane_mix, mix);
        assert_eq!(project.sample_regions, sample_regions);
        assert_eq!(project.sample_bank().regions, sample_regions);
        assert_eq!(project.selected_pattern, 1);
        assert_eq!(project.chain, chain);
        assert_eq!(project.patterns[1].automation[1].fixed_byte(), Some(0x00));
        for (original, loaded) in patterns.iter().zip(&project.patterns) {
            assert_eq!(original.rate, loaded.rate);
            assert_eq!(original.swing, loaded.swing);
            assert_eq!(original.lanes, loaded.lanes);
            assert_eq!(original.automation, loaded.automation);
        }
        assert_eq!(
            project.patterns[1].automation[0].roll.to_bits(),
            (-0.0_f32).to_bits()
        );
        let restored = project.samples[2].as_ref().unwrap();
        assert_eq!(restored.samples, pcm);
        assert_eq!(restored.peaks, samples[2].as_ref().unwrap().peaks);
        assert_eq!(
            restored.c4_rate_hz.to_bits(),
            samples[2].as_ref().unwrap().c4_rate_hz.to_bits()
        );
    }

    #[test]
    fn legacy_capture_pcm_decodes_at_the_limit_and_rejects_the_next_frame() {
        let prefix = r#"{"rate":48000,"frames":["#;
        let frames = "[100,-200],".repeat(MAX_CAPTURE_FRAMES - 1);
        let valid = format!("{prefix}{frames}[300,-400]]}}");
        let capture: CaptureFile = serde_json::from_str(&valid).unwrap();
        let take = capture.decode().unwrap();
        assert_eq!(take.rate, 48_000);
        assert_eq!(take.frames.len(), MAX_CAPTURE_FRAMES);
        assert_eq!(take.frames[0], [100, -200]);
        assert_eq!(take.frames[MAX_CAPTURE_FRAMES - 1], [300, -400]);
        let oversized = format!("{prefix}{frames}[300,-400],[500,-600]]}}");
        let error = serde_json::from_str::<CaptureFile>(&oversized).err().unwrap();
        assert!(error.to_string().contains("frame budget"));
    }

    #[test]
    fn bbus_settings_keep_old_projects_free_running_and_round_trip() {
        let legacy: InstrumentFile = serde_json::from_str(r#"{"routing":{}}"#).unwrap_or_default();
        assert_eq!(legacy.bbus, BbusSettings::default());
        assert_eq!(BbusSettings::default().grit_lanes(), 0b11_1111);
        let mut locked = BbusSettings { cycle_lock: true, ..BbusSettings::default() };
        assert_eq!(locked.grit_lanes(), 0);
        locked.grit[2] = true;
        assert_eq!(locked.grit_lanes(), 0b100);
        let json = serde_json::to_string(&locked).unwrap();
        assert_eq!(serde_json::from_str::<BbusSettings>(&json).unwrap(), locked);
        let file = InstrumentFile { bbus: BbusSettings::default(), ..InstrumentFile::default() };
        assert!(!serde_json::to_string(&file).unwrap().contains("bbus"));
        assert!(!legacy.otis.warm_tables);
        assert_eq!(legacy.oversample.grit_lanes(), 0b11_1111);
        let mut oversample = OversampleSettings { enabled: true, ..OversampleSettings::default() };
        assert_eq!(oversample.grit_lanes(), 0);
        oversample.grit[4] = true;
        assert_eq!(oversample.grit_lanes(), 0b1_0000);
        let warm = OtisSettings { warm_tables: true };
        let json = serde_json::to_string(&warm).unwrap();
        assert_eq!(serde_json::from_str::<OtisSettings>(&json).unwrap(), warm);
    }

    #[test]
    fn kit_lane_velocity_curve_round_trips_and_legacy_lanes_stay_on_chip() {
        let legacy: StoredKitLane =
            serde_json::from_str(r#"{"bytebus":false,"clock":1,"drop_samples":true}"#).unwrap();
        assert_eq!(legacy.0.velocity, VelocityCurve::Chip);
        assert!(legacy.0.raw_output);
        assert_eq!(legacy.0.filter_taper, FilterTaper::Squared);
        let fresh = serde_json::to_string(&StoredKitLane(KitLane::default())).unwrap();
        let fresh: StoredKitLane = serde_json::from_str(&fresh).unwrap();
        assert!(!fresh.0.raw_output);
        assert_eq!(fresh.0.filter_taper, FilterTaper::Linear);
        assert_eq!((legacy.0.clock, legacy.0.drop_samples), (1, true));
        let chip = serde_json::to_string(&StoredKitLane(KitLane::default())).unwrap();
        assert!(!chip.contains("velocity"), "{chip}");
        let soft = KitLane { velocity: VelocityCurve::Soft, ..KitLane::default() };
        let json = serde_json::to_string(&StoredKitLane(soft)).unwrap();
        assert!(json.contains(r#""velocity":"Soft""#), "{json}");
        assert_eq!(serde_json::from_str::<StoredKitLane>(&json).unwrap().0, soft);
    }

    #[test]
    fn full_wave8_kit_and_take_share_a_bounded_project_and_reload() {
        use crate::export::{ExportJob, ExportRequest, ExportSettings};
        use leviathan_core::NoteCell;
        use leviathan_core::rack_routing::LaneInput;

        let path = std::env::temp_dir().join(format!("leviathan-wave8-{}.leviathan.json", std::process::id()));
        let mut patterns = vec![Pattern::default(); PATTERN_BANK_CAPACITY];
        patterns[0].automation[1].byte_override = true;
        patterns[0].automation[2].byte_word = 0x80;
        let mut instruments = InstrumentSettings {
            take: Take::new(96_000, vec![[i16::MIN, i16::MAX]; MAX_CAPTURE_FRAMES]),
            echo: echo::Params { esp: esp::Mode::Rate128, sync: echo::Sync::ByteBus, echo_mix: 0.4, ..echo::Params::default() },
            ..InstrumentSettings::default()
        };
        for i in 0..SLOTS {
            instruments.kit.samples[i] = Some(KitSample::new(format!("Drum {i}"), 44_100 + i as u32, vec![2048 - i as i16; MAX_KIT_FRAMES / SLOTS]).unwrap());
        }
        instruments.kit.params[7].reverse = true;
        instruments.kit.lanes[0].bytebus = true;
        instruments.routing.inputs[4] = LaneInput::Capture;
        let mut patches = default_lane_patches();
        patches[0] = LanePatch::for_machine(MachineKind::OtisKit);
        patterns[0].length = 1;
        for lane in [0, 4] {
            patterns[0].lanes[lane][0] = NoteCell {
                active: true, midi: 60, velocity: 112, ..NoteCell::default()
            };
        }
        let bank_pcm = vec![i16::MIN; MAX_WAVE_LEN];
        let bank = DecodedWav {
            name: "Bank".into(),
            peaks: sample_peaks(&bank_pcm),
            samples: bank_pcm.into(),
            c4_rate_hz: 48_000.0,
            original_frames: MAX_WAVE_LEN,
            original_sr: 48_000,
        };
        let source = Project {
            bpm: 120.0, master_send: 0.0, delay_mode: Default::default(), bender_params: Default::default(),
            lane_mix: LaneMix::default(), lane_patches: patches, patterns, selected_pattern: 0,
            chain: PatternChain::default(), samples: std::array::from_fn(|_| Some(bank.clone())), sample_regions: Default::default(),
            custom_shader: None, voice_calibration: false, instruments,
        };
        save(&path, source).unwrap();
        let bytes_on_disk = fs::metadata(&path).unwrap().len();
        let mut loaded = load(&path).unwrap();
        assert!(bytes_on_disk < MAX_PROJECT_BYTES);
        let take = loaded.instruments.take.as_ref().unwrap();
        assert_eq!(take.frames.len(), MAX_CAPTURE_FRAMES);
        assert!(take.frames.iter().all(|&frame| frame == [i16::MIN, i16::MAX]));
        assert_eq!(loaded.instruments.echo.esp, esp::Mode::Rate128);
        assert_eq!(loaded.patterns[0].automation[1].fixed_byte(), Some(0));
        assert_eq!(loaded.patterns[0].automation[2].fixed_byte(), Some(0x80));
        assert_eq!(loaded.instruments.kit.samples[7].as_ref().unwrap().pcm, vec![2041; MAX_KIT_FRAMES / SLOTS]);
        assert!(loaded.instruments.kit.params[7].reverse && loaded.instruments.kit.lanes[0].bytebus);
        let old = loaded.instruments.kit.samples[0].as_ref().unwrap().clone();
        let too_big = KitSample::new("Oversized".into(), 48_000, vec![0; old.pcm.len() + 1]).unwrap();
        assert!(loaded.instruments.kit.replace(0, too_big).is_err());
        assert!(Arc::ptr_eq(&old, loaded.instruments.kit.samples[0].as_ref().unwrap()));
        let bounce = path.with_extension("wav");
        ExportJob::start(loaded, false, ExportRequest {
            settings: ExportSettings { tail_seconds: 0.0, ..ExportSettings::default() },
            destination: bounce.clone(),
        }).unwrap().finish().unwrap();
        assert_eq!(fs::metadata(&bounce).unwrap().len(), 44 + 6000 * 4);
        fs::remove_file(&bounce).unwrap();
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_incompatible_version_invalid_chain_and_wrong_row_count() {
        let mut project = ProjectFile {
            version: PROJECT_VERSION + 1,
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_patches: None,
            lane_mix: LaneMix::default(),
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: vec![1],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        };
        let mut json = serde_json::to_value(&project).unwrap();
        assert!(project_error(&json).contains("version"));
        project.version = PROJECT_VERSION;
        json = serde_json::to_value(&project).unwrap();
        assert!(project_error(&json).contains("chain"));
        json["patterns"][0]["lanes"][0]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(project_error(&json).contains("64 rows"));
    }

    #[test]
    fn rejects_out_of_range_sound_lock_machine() {
        let mut pat = Pattern::default();
        pat.lanes[0][0].lock.machine = leviathan_dsp::MACHINE_COUNT as i16;
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.0,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_patches: None,
            lane_mix: LaneMix::default(),
            patterns: vec![pat],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        };
        let json = serde_json::to_value(&project).unwrap();
        assert!(project_error(&json).contains("Invalid sound lock"));
    }

    #[test]
    fn expansion_patches_and_step_machine_ids_round_trip() {
        let path =
            std::env::temp_dir().join(format!("leviathan-expansion-{}.json", std::process::id()));
        let engines = [
            MachineKind::CzPhaseDist,
            MachineKind::PolivoksSub,
            MachineKind::ByteBusR2r,
            MachineKind::ElectroPerc,
            MachineKind::WestCoastLpg,
            MachineKind::OtisTranswave,
        ];
        let mut pattern = Pattern::default();
        let mut patches = default_lane_patches();
        for (lane, engine) in engines.into_iter().enumerate() {
            patches[lane] = LanePatch::for_machine(engine);
            patches[lane].params[0] = (lane + 1) as f32 / NUM_LANES as f32;
            pattern.lanes[lane][0].lock.machine = engines[(lane + 1) % engines.len()] as i16;
            pattern.lanes[lane][0].lock.mask = 1 << 3;
            pattern.lanes[lane][0].lock.values[3] = 0.75;
        }
        save(
            &path,
            Project {
                bpm: 120.0,
                master_send: 0.0,
                delay_mode: Default::default(),
                bender_params: Default::default(),
                lane_patches: patches,
                lane_mix: LaneMix::default(),
                patterns: vec![pattern.clone()],
                selected_pattern: 0,
                chain: PatternChain::default(),
                samples: core::array::from_fn(|_| None),
                sample_regions: Default::default(),
                custom_shader: None,
                voice_calibration: false,
                instruments: Default::default(),
            },
        )
        .unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(stored.contains(r#""machine":"bbus""#));
        let project = load(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(project.lane_patches, patches);
        assert_eq!(project.patterns[0].lanes, pattern.lanes);
    }

    #[test]
    fn older_projects_default_to_straight_timing_and_the_original_mix() {
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.5,
            delay_mode: Default::default(),
            bender_params: Default::default(),
            lane_patches: None,
            lane_mix: LaneMix {
                mute_mask: 1,
                solo_mask: 2,
                ..LaneMix::default()
            },
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
            voice_calibration: false,
            instruments: Default::default(),
        };
        let mut json = serde_json::to_value(project).unwrap();
        let fields = json.as_object_mut().unwrap();
        for field in [
            "master_send",
            "delay_mode",
            "bender_params",
            "mute_mask",
            "solo_mask",
            "channels",
            "sample_regions",
        ] {
            assert!(fields.remove(field).is_some());
        }
        json["patterns"][0]["automation"][1]["byte_word"] = serde_json::json!(0x70);
        for row in json["patterns"][0]["automation"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().remove("byte_override");
        }
        let pattern = json["patterns"][0].as_object_mut().unwrap();
        pattern.remove("swing");
        for lane in pattern.get_mut("lanes").unwrap().as_array_mut().unwrap() {
            for cell in lane.as_array_mut().unwrap() {
                let cell = cell.as_object_mut().unwrap();
                cell.remove("micro_offset");
                cell.remove("retrig");
            }
        }
        let old: ProjectFile = serde_json::from_value(json.clone()).unwrap();
        let old = old.into_project().unwrap();
        assert_eq!(old.master_send, 0.0);
        assert_eq!(old.delay_mode, DelayMode::Slapback);
        assert_eq!(old.bender_params, BenderParams::default());
        assert_eq!(old.lane_mix, LaneMix::default());
        assert_eq!(old.sample_regions, [SampleRegion::default(); NUM_BANKS]);
        assert_eq!(old.patterns[0].swing, 0.5);
        assert_eq!(old.patterns[0].lanes[0][0], NoteCell::default());
        assert_eq!(old.patterns[0].automation[0].fixed_byte(), None);
        assert_eq!(old.patterns[0].automation[1].fixed_byte(), Some(0x70));
        json["patterns"][0]["swing"] = serde_json::json!(0.8);
        assert!(project_error(&json).contains("swing"));
        json["patterns"][0]["swing"] = serde_json::json!(0.5);
        json["patterns"][0]["lanes"][0][0]["micro_offset"] = serde_json::json!(-25);
        assert!(project_error(&json).contains("nudge"));
        json["patterns"][0]["lanes"][0][0]["micro_offset"] = serde_json::json!(0);
        json["patterns"][0]["lanes"][0][0]["retrig"] = serde_json::json!(0);
        assert!(project_error(&json).contains("retrigger count"));
        json["patterns"][0]["lanes"][0][0]["retrig"] = serde_json::json!(1);
        json["channels"] = serde_json::to_value(LaneMix::default().channels).unwrap();
        for channel in json["channels"].as_array_mut().unwrap() {
            assert!(channel.as_object_mut().unwrap().remove("fx_send").is_some());
        }
        json["channels"][0]["delay_send"] = serde_json::json!(0.72);
        let pre_bend = serde_json::from_value::<ProjectFile>(json.clone())
            .unwrap()
            .into_project()
            .unwrap();
        assert_eq!(pre_bend.lane_mix.channels[0].0.delay_send, 0.72);
        assert!(pre_bend.lane_mix.dsp_channels().iter().all(|channel| channel.fx_send == 0.0));
        for (field, value) in [("level", 1.251), ("pan", -1.01), ("delay_send", 1.01), ("fx_send", 1.01)] {
            let mut invalid = json.clone();
            invalid["channels"][0][field] = serde_json::json!(value);
            assert!(project_error(&invalid).contains("mixer settings"));
        }
        json["sample_regions"] = serde_json::json!([
            {"trim_start": -0.1, "trim_end": 1.0, "loop_enabled": true},
            {"trim_start": 0.0, "trim_end": 1.0, "loop_enabled": false},
            {"trim_start": 0.0, "trim_end": 1.0, "loop_enabled": false},
            {"trim_start": 0.0, "trim_end": 1.0, "loop_enabled": false}
        ]);
        assert!(project_error(&json).contains("invalid sample trim"));
        json.as_object_mut().unwrap().remove("sample_regions");
        json["solo_mask"] = serde_json::json!(0x80);
        assert!(project_error(&json).contains("mute or solo mask"));
        json["solo_mask"] = serde_json::json!(0);
        json["bender_params"] = serde_json::json!({
            "shift_norm": 0.5, "decay": 0.4, "feedback": 1.1, "freeze": false
        });
        assert!(project_error(&json).contains("BEND shift, feedback and decay"));
        json.as_object_mut().unwrap().remove("bender_params");
        json["master_send"] = serde_json::json!(1.25);
        assert!(project_error(&json).contains("Master delay send"));
    }

    #[test]
    fn custom_shader_and_lane_mix_round_trip_in_the_same_project() {
        let path = std::env::temp_dir()
            .join(format!("leviathan-shader-rt-{}.json", std::process::id()));
        let patterns = [Pattern::default()];
        let chain = PatternChain::default();
        let samples = core::array::from_fn(|_| None);
        let shader = CustomShader {
            name: "starter_custom.wgsl".to_string(),
            wgsl: leviathan_gpu::STARTER_SHADER_TEMPLATE_WGSL.to_string(),
        };
        let lane_mix = LaneMix {
            mute_mask: 0b10,
            solo_mask: 0b11,
        ..LaneMix::default()
        };

        save(
            &path,
            Project {
                bpm: 120.0,
                master_send: 0.25,
                delay_mode: Default::default(),
                bender_params: Default::default(),
                lane_patches: default_lane_patches(),
                lane_mix,
                patterns: patterns.to_vec(),
                selected_pattern: 0,
                chain,
                samples,
                sample_regions: Default::default(),
                custom_shader: Some(shader.clone()),
                voice_calibration: false,
                instruments: Default::default(),
            },
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.custom_shader, Some(shader));
        assert_eq!(loaded.lane_mix, lane_mix);

        let raw_bytes = fs::read(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&raw_bytes).unwrap();
        json.as_object_mut().unwrap().remove("custom_shader");
        let legacy: ProjectFile = serde_json::from_value(json.clone()).unwrap();
        let legacy = legacy.into_project().unwrap();
        assert_eq!(legacy.custom_shader, None);
        assert_eq!(legacy.lane_mix, lane_mix);

        json["custom_shader"] = serde_json::json!({
            "name": "broken.wgsl",
            "wgsl": "fn fs_main() {"
        });
        assert!(project_error(&json).contains("Unclosed"));

        let _ = fs::remove_file(&path);
    }

    fn project_error(json: &serde_json::Value) -> String {
        let decoded: Result<ProjectFile, _> = serde_json::from_value(json.clone());
        match decoded {
            Ok(file) => file.into_project().err().unwrap(),
            Err(error) => error.to_string(),
        }
    }
}

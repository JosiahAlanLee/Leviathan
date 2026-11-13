use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use leviathan_core::{
    Pattern, PatternChain, MAX_CHAIN_ENTRIES, MAX_NUDGE, MAX_RETRIG, MAX_SWING, NUM_LANES,
    ROWS_PER_PATTERN, SOUND_PARAMS,
};
use leviathan_dsp::sampler::{SampleBank, SampleRegion, MAX_WAVE_LEN, NUM_BANKS};
use leviathan_dsp::{default_lane_patches, LanePatch, MachineKind};
use serde::{Deserialize, Serialize};

use crate::lane_mix::LaneMix;
use crate::wav::{sample_peaks, DecodedWav};

pub const PATTERN_BANK_CAPACITY: usize = 16;
const PROJECT_VERSION: u32 = 1;
const MAX_PROJECT_BYTES: u64 = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomShader {
    pub name: String,
    pub wgsl: String,
}
pub struct Project {
    pub bpm: f64,
    pub master_send: f32,
    pub lane_mix: LaneMix,
    pub lane_patches: [LanePatch; NUM_LANES],
    pub patterns: Vec<Pattern>,
    pub selected_pattern: usize,
    pub chain: PatternChain,
    pub samples: [Option<DecodedWav>; NUM_BANKS],
    pub sample_regions: [SampleRegion; NUM_BANKS],
    pub custom_shader: Option<CustomShader>,
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
        if self.version != PROJECT_VERSION {
            return Err(format!("Unsupported project version {}", self.version));
        }
        if !(40.0..=240.0).contains(&self.bpm) {
            return Err("BPM must be between 40 and 240".to_string());
        }
        if !(0.0..=1.0).contains(&self.master_send) {
            return Err("Master delay send must be between 0 and 1".to_string());
        }
        if !self.lane_mix.is_valid() {
            return Err("Invalid lane mute or solo mask or mixer settings".to_string());
        }
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
            lane_mix: self.lane_mix,
            lane_patches,
            patterns: self.patterns,
            selected_pattern: self.selected_pattern,
            chain,
            samples,
            sample_regions,
            custom_shader: self.custom_shader,
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
    let project = ProjectFile {
        version: PROJECT_VERSION,
        bpm: project.bpm,
        master_send: project.master_send,
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
        let mut patterns = [Pattern::default(), Pattern::default()];
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
        };
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
        let snapshot = |bpm, master_send, selected_pattern, lane_mix| Project {
            bpm,
            master_send,
            lane_patches: default_lane_patches(),
            lane_mix,
            selected_pattern,
            chain,
            patterns: patterns.to_vec(),
            samples: samples.clone(),
            sample_regions,
            custom_shader: None,
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
        save(&path, snapshot(bpm, send, 1, mix)).unwrap();
        let project = load(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(project.bpm.to_bits(), bpm.to_bits());
        assert_eq!(project.master_send.to_bits(), send.to_bits());
        assert_eq!(project.lane_mix, mix);
        assert_eq!(project.sample_regions, sample_regions);
        assert_eq!(project.sample_bank().regions, sample_regions);
        assert_eq!(project.selected_pattern, 1);
        assert_eq!(project.chain, chain);
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
    fn rejects_incompatible_version_invalid_chain_and_wrong_row_count() {
        let mut project = ProjectFile {
            version: PROJECT_VERSION + 1,
            bpm: 120.0,
            master_send: 0.0,
            lane_patches: None,
            lane_mix: LaneMix::default(),
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: vec![1],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
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
            lane_patches: None,
            lane_mix: LaneMix::default(),
            patterns: vec![pat],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
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
            MachineKind::VastFunSynth,
        ];
        let mut pattern = Pattern::default();
        let mut patches = default_lane_patches();
        for (lane, engine) in engines.into_iter().enumerate() {
            patches[lane] = LanePatch::for_machine(engine);
            patches[lane].params[0] = (lane + 1) as f32 / 4.0;
            pattern.lanes[lane][0].lock.machine = engines[(lane + 1) % engines.len()] as i16;
            pattern.lanes[lane][0].lock.mask = 1 << 3;
            pattern.lanes[lane][0].lock.values[3] = 0.75;
        }
        save(
            &path,
            Project {
                bpm: 120.0,
                master_send: 0.0,
                lane_patches: patches,
                lane_mix: LaneMix::default(),
                patterns: vec![pattern.clone()],
                selected_pattern: 0,
                chain: PatternChain::default(),
                samples: core::array::from_fn(|_| None),
                sample_regions: Default::default(),
                custom_shader: None,
            },
        )
        .unwrap();
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
        };
        let mut json = serde_json::to_value(project).unwrap();
        let fields = json.as_object_mut().unwrap();
        for field in [
            "master_send",
            "mute_mask",
            "solo_mask",
            "channels",
            "sample_regions",
        ] {
            assert!(fields.remove(field).is_some());
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
        assert_eq!(old.lane_mix, LaneMix::default());
        assert_eq!(old.sample_regions, [SampleRegion::default(); NUM_BANKS]);
        assert_eq!(old.patterns[0].swing, 0.5);
        assert_eq!(old.patterns[0].lanes[0][0], NoteCell::default());
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
        for (field, value) in [("level", 1.251), ("pan", -1.01), ("delay_send", 1.01)] {
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
                lane_patches: default_lane_patches(),
                lane_mix,
                patterns: patterns.to_vec(),
                selected_pattern: 0,
                chain,
                samples,
                sample_regions: Default::default(),
                custom_shader: Some(shader.clone()),
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

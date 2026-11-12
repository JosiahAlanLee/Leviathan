use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::lane_mix::LaneMix;
use crate::shader::CustomShader;
use leviathan_core::{
    Pattern, PatternChain, MAX_CHAIN_ENTRIES, MAX_NUDGE, MAX_RETRIG, MAX_SWING, NUM_LANES,
    ROWS_PER_PATTERN, SOUND_PARAMS,
};
use leviathan_dsp::{default_lane_patches, LanePatch, MachineKind};
use leviathan_dsp::sampler::{SampleBank, SampleRegion, MAX_WAVE_LEN, NUM_BANKS};

pub const PROJECT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedWav {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredLanePatch {
    machine: String,
    params: [f32; SOUND_PARAMS],
}

impl From<LanePatch> for StoredLanePatch {
    fn from(patch: LanePatch) -> Self {
        Self {
            machine: patch.machine.slug().to_owned(),
            params: patch.params,
        }
    }
}

fn default_stored_lane_patches() -> [StoredLanePatch; NUM_LANES] {
    default_lane_patches().map(StoredLanePatch::from)
}

#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SampleFile {
    path: String,
    data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredSampleRegion {
    start: usize,
    end: usize,
}

impl Default for StoredSampleRegion {
    fn default() -> Self {
        Self {
            start: 0,
            end: MAX_WAVE_LEN,
        }
    }
}

impl From<SampleRegion> for StoredSampleRegion {
    fn from(region: SampleRegion) -> Self {
        Self {
            start: region.start,
            end: region.end,
        }
    }
}

impl From<StoredSampleRegion> for SampleRegion {
    fn from(stored: StoredSampleRegion) -> Self {
        Self {
            start: stored.start,
            end: stored.end,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f64,
    master_send: f32,
    #[serde(default)]
    lane_mix: LaneMix,
    #[serde(default = "default_stored_lane_patches")]
    lane_patches: [StoredLanePatch; NUM_LANES],
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: PatternChain,
    sample_banks: [Option<SampleFile>; NUM_BANKS],
    sample_regions: [StoredSampleRegion; NUM_BANKS],
    custom_shader: Option<CustomShader>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            master_send: 0.25,
            lane_mix: LaneMix::default(),
            lane_patches: default_lane_patches(),
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions: Default::default(),
            custom_shader: None,
        }
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
        let mut lane_patches = [LanePatch::default(); NUM_LANES];
        for (idx, stored) in self.lane_patches.into_iter().enumerate() {
            let machine = MachineKind::from_slug(&stored.machine)
                .ok_or_else(|| "Invalid lane patch".to_string())?;
            let patch = LanePatch {
                machine,
                params: stored.params,
            };
            if !patch.is_valid() {
                return Err("Invalid lane patch".to_string());
            }
            lane_patches[idx] = patch;
        }
        if self.patterns.is_empty() {
            return Err("Project must contain at least one pattern".to_string());
        }
        if self.selected_pattern >= self.patterns.len() {
            return Err("Selected pattern index out of range".to_string());
        }

        for (pattern_idx, pattern) in self.patterns.iter().enumerate() {
            for (lane_idx, lane) in pattern.lanes.iter().enumerate() {
                for (row_idx, row) in lane.rows.iter().enumerate() {
                    if row.note > 127 {
                        return Err(format!(
                            "Invalid note {} at pattern {} lane {} row {}",
                            row.note, pattern_idx, lane_idx, row_idx
                        ));
                    }
                    if row.velocity > 127 {
                        return Err(format!(
                            "Invalid velocity {} at pattern {} lane {} row {}",
                            row.velocity, pattern_idx, lane_idx, row_idx
                        ));
                    }
                    if !(-MAX_NUDGE..=MAX_NUDGE).contains(&row.nudge) {
                        return Err(format!(
                            "Invalid nudge {} at pattern {} lane {} row {}",
                            row.nudge, pattern_idx, lane_idx, row_idx
                        ));
                    }
                    if row.retrig > MAX_RETRIG {
                        return Err(format!(
                            "Invalid retrig {} at pattern {} lane {} row {}",
                            row.retrig, pattern_idx, lane_idx, row_idx
                        ));
                    }
                }
            }
        }

        if self.chain.entries.len() > MAX_CHAIN_ENTRIES {
            return Err("Chain length exceeds maximum allowed".to_string());
        }

        for &entry in &self.chain.entries {
            if entry as usize >= self.patterns.len() {
                return Err("Chain entry references out of bounds pattern".to_string());
            }
        }

        if !(-MAX_SWING..=MAX_SWING).contains(&self.chain.swing) {
            return Err("Chain swing must be between -0.5 and 0.5".to_string());
        }

        let mut samples: [Option<DecodedWav>; NUM_BANKS] = Default::default();
        for (idx, bank_opt) in self.sample_banks.into_iter().enumerate() {
            if let Some(sample_file) = bank_opt {
                let decoded = decode_wav_bytes(&sample_file.data)?;
                samples[idx] = Some(decoded);
            }
        }

        let sample_regions = self.sample_regions.map(SampleRegion::from);
        for (idx, region) in sample_regions.iter().enumerate() {
            if !region.is_valid() {
                return Err(format!("Invalid sample region for bank {}", idx));
            }
        }

        if let Some(shader) = &self.custom_shader {
            if !shader.is_valid() {
                return Err("Invalid custom shader".to_string());
            }
        }

        Ok(Project {
            bpm: self.bpm,
            master_send: self.master_send,
            lane_mix: self.lane_mix,
            lane_patches,
            patterns: self.patterns,
            selected_pattern: self.selected_pattern,
            chain: self.chain,
            samples,
            sample_regions,
            custom_shader: self.custom_shader,
        })
    }
}

pub fn save(path: &Path, project: &Project) -> Result<(), String> {
    let sample_banks = core::array::from_fn(|idx| {
        project.samples[idx].as_ref().map(|wav| SampleFile {
            path: format!("bank_{}.wav", idx),
            data: encode_wav_bytes(wav),
        })
    });
    let sample_regions = project.sample_regions.map(StoredSampleRegion::from);

    let file = ProjectFile {
        version: PROJECT_VERSION,
        bpm: project.bpm,
        master_send: project.master_send,
        lane_mix: project.lane_mix,
        lane_patches: project.lane_patches.map(StoredLanePatch::from),
        patterns: project.patterns.clone(),
        selected_pattern: project.selected_pattern,
        chain: project.chain,
        sample_banks,
        sample_regions,
        custom_shader: project.custom_shader.clone(),
    };

    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|err| format!("Failed to serialize project: {}", err))?;
    fs::write(path, bytes).map_err(|err| format!("Failed to write project file: {}", err))?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Project, String> {
    let bytes = fs::read(path).map_err(|err| format!("Failed to read project file: {}", err))?;
    let file: ProjectFile = serde_json::from_slice(&bytes)
        .map_err(|err| format!("Failed to parse project JSON: {}", err))?;
    file.into_project()
}

fn decode_wav_bytes(bytes: &[u8]) -> Result<DecodedWav, String> {
    let mut reader = hound::WavReader::new(bytes)
        .map_err(|err| format!("Failed to parse WAV header: {}", err))?;
    let spec = reader.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<f32>, _>>()
            .map_err(|err| format!("Failed to read float samples: {}", err))?,
        hound::SampleFormat::Int => {
            let max_val = 1u32.checked_shl(spec.bits_per_sample - 1).unwrap_or(1) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max_val))
                .collect::<Result<Vec<f32>, _>>()
                .map_err(|err| format!("Failed to read int samples: {}", err))?
        }
    };
    Ok(DecodedWav {
        samples,
        sample_rate: spec.sample_rate,
        channels: spec.channels as usize,
    })
}

fn encode_wav_bytes(wav: &DecodedWav) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: wav.channels as u16,
        sample_rate: wav.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("valid wav spec");
    for &sample in &wav.samples {
        writer.write_sample(sample).expect("valid float sample");
    }
    writer.finalize().expect("finalize wav file");
    cursor.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane_mix::LaneMix;
    use leviathan_core::NoteCell;

    fn project_error(val: &serde_json::Value) -> String {
        let file: ProjectFile = serde_json::from_value(val.clone()).unwrap();
        file.into_project().unwrap_err()
    }

    #[test]
    fn project_file_round_trip() {
        let path = std::env::temp_dir().join(format!("leviathan-proj-rt-{}.json", std::process::id()));
        let mut patterns = vec![Pattern::default(), Pattern::default()];
        patterns[0].lanes[0].rows[0] = NoteCell {
            note: 60,
            velocity: 100,
            nudge: 0.1,
            retrig: 2,
        };

        let chain = PatternChain {
            entries: vec![0, 1, 0],
            swing: 0.25,
        };

        let samples: [Option<DecodedWav>; NUM_BANKS] = core::array::from_fn(|idx| {
            if idx == 0 {
                Some(DecodedWav {
                    samples: vec![0.0, 0.5, -0.5, 0.0],
                    sample_rate: 44100,
                    channels: 1,
                })
            } else {
                None
            }
        });

        let mut sample_regions = [SampleRegion::default(); NUM_BANKS];
        sample_regions[0] = SampleRegion {
            start: 1,
            end: 3,
        };

        let snapshot = |bpm, master_send, selected_pattern, lane_mix| Project {
            bpm,
            master_send,
            lane_mix,
            lane_patches: default_lane_patches(),
            selected_pattern,
            chain: chain.clone(),
            patterns: patterns.to_vec(),
            samples: samples.clone(),
            sample_regions,
            custom_shader: None,
        };

        let original = snapshot(132.5, 0.75, 1, LaneMix::default());
        save(&path, &original).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, original);

        let mut updated_mix = LaneMix::default();
        updated_mix.set_level(0, 0.8);
        updated_mix.set_pan(1, -0.5);
        updated_mix.set_delay_send(2, 0.3);
        updated_mix.mute_mask = 5;
        updated_mix.solo_mask = 2;

        let original_updated = snapshot(132.5, 0.75, 1, updated_mix);
        save(&path, &original_updated).unwrap();
        let loaded_updated = load(&path).unwrap();
        assert_eq!(loaded_updated, original_updated);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn invalid_project_values_are_rejected() {
        let pat = Pattern::default();
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.0,
            lane_mix: LaneMix::default(),
            lane_patches: default_stored_lane_patches(),
            patterns: vec![pat],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
        };
        let mut json = serde_json::to_value(&project).unwrap();

        json["version"] = serde_json::json!(999);
        assert!(project_error(&json).contains("version"));
        json["version"] = serde_json::json!(PROJECT_VERSION);

        json["bpm"] = serde_json::json!(39.9);
        assert!(project_error(&json).contains("BPM"));
        json["bpm"] = serde_json::json!(240.1);
        assert!(project_error(&json).contains("BPM"));
        json["bpm"] = serde_json::json!(120.0);

        json["master_send"] = serde_json::json!(-0.01);
        assert!(project_error(&json).contains("Master delay send"));
        json["master_send"] = serde_json::json!(1.01);
        assert!(project_error(&json).contains("Master delay send"));
        json["master_send"] = serde_json::json!(0.0);

        json["patterns"] = serde_json::json!([]);
        assert!(project_error(&json).contains("at least one pattern"));
        json["patterns"] = serde_json::json!([Pattern::default()]);

        json["selected_pattern"] = serde_json::json!(1);
        assert!(project_error(&json).contains("selected pattern index out of range"));
        json["selected_pattern"] = serde_json::json!(0);

        json["chain"] = serde_json::json!([1]);
        assert!(project_error(&json).contains("out of bounds pattern"));
        json["chain"] = serde_json::json!([]);

        json["patterns"][0]["lanes"][0][0]["note"] = serde_json::json!(128);
        assert!(project_error(&json).contains("note"));
        json["patterns"][0]["lanes"][0][0]["note"] = serde_json::json!(60);

        json["patterns"][0]["lanes"][0][0]["velocity"] = serde_json::json!(128);
        assert!(project_error(&json).contains("velocity"));
        json["patterns"][0]["lanes"][0][0]["velocity"] = serde_json::json!(100);

        json["patterns"][0]["lanes"][0][0]["nudge"] = serde_json::json!(0.51);
        assert!(project_error(&json).contains("nudge"));
        json["patterns"][0]["lanes"][0][0]["nudge"] = serde_json::json!(0.0);

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
    fn rejects_out_of_range_sound_lock_machine() {
        let mut pat = Pattern::default();
        pat.lanes[0][0].lock.machine = 7;
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.0,
            lane_mix: LaneMix::default(),
            lane_patches: default_stored_lane_patches(),
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
    fn older_projects_default_to_straight_timing_and_the_original_mix() {
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.5,
            lane_mix: LaneMix {
                mute_mask: 1,
                solo_mask: 2,
                ..LaneMix::default()
            },
            lane_patches: default_stored_lane_patches(),
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
        assert_eq!(old.lane_patches, default_lane_patches());
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
        let shader = CustomShader {
            code: "void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(1.0); }".to_string(),
        };
        let mut lane_mix = LaneMix::default();
        lane_mix.set_level(0, 0.45);
        lane_mix.set_pan(1, 0.3);
        lane_mix.set_delay_send(2, 0.75);

        let project = Project {
            bpm: 128.0,
            master_send: 0.4,
            lane_mix,
            lane_patches: default_lane_patches(),
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: Default::default(),
            sample_regions: Default::default(),
            custom_shader: Some(shader.clone()),
        };

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.custom_shader, Some(shader));
        assert_eq!(loaded.lane_mix, lane_mix);
        assert_eq!(loaded.lane_patches, default_lane_patches());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn lane_patches_round_trip_and_unknown_machine_slug_is_rejected() {
        let path = std::env::temp_dir()
            .join(format!("leviathan-lane-patches-rt-{}.json", std::process::id()));
        let mut lane_patches = default_lane_patches();
        lane_patches[0] = LanePatch {
            machine: MachineKind::FourOpFm,
            params: [0.5; SOUND_PARAMS],
        };
        lane_patches[1] = LanePatch {
            machine: MachineKind::FourOpFm,
            params: [0.8; SOUND_PARAMS],
        };
        lane_patches[2] = LanePatch {
            machine: MachineKind::ChordStab,
            params: [0.2; SOUND_PARAMS],
        };

        let project = Project {
            bpm: 120.0,
            master_send: 0.25,
            lane_mix: LaneMix::default(),
            lane_patches: lane_patches.clone(),
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: PatternChain::default(),
            samples: core::array::from_fn(|_| None),
            sample_regions: Default::default(),
            custom_shader: None,
        };

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.lane_patches, lane_patches);

        let raw_bytes = fs::read(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&raw_bytes).unwrap();
        let obj = json.as_object_mut().unwrap();
        let patches_arr = obj.get_mut("lane_patches").unwrap().as_array_mut().unwrap();
        patches_arr[0]["machine"] = serde_json::json!("unknown_synth");

        assert!(project_error(&json).contains("Invalid lane patch"));

        let _ = fs::remove_file(&path);
    }
}

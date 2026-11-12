use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use leviathan_core::{Pattern, PatternChain, MAX_CHAIN_ENTRIES, ROWS_PER_PATTERN};
use leviathan_dsp::sampler::{MAX_WAVE_LEN, NUM_BANKS};
use serde::{Deserialize, Serialize};

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
    pub patterns: Vec<Pattern>,
    pub selected_pattern: usize,
    pub chain: PatternChain,
    pub samples: [Option<DecodedWav>; NUM_BANKS],
    pub custom_shader: Option<CustomShader>,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    version: u32,
    bpm: f64,
    #[serde(default)]
    master_send: f32,
    patterns: Vec<Pattern>,
    selected_pattern: usize,
    chain: Vec<u8>,
    sample_banks: [Option<SampleFile>; NUM_BANKS],
    #[serde(default)]
    custom_shader: Option<CustomShader>,
}

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
            patterns: self.patterns,
            selected_pattern: self.selected_pattern,
            chain,
            samples,
        custom_shader: self.custom_shader,
        })
    }
}

fn validate_pattern(pattern: &Pattern) -> Result<(), &'static str> {
    if !(1..=ROWS_PER_PATTERN).contains(&pattern.length) {
        return Err("Invalid pattern length");
    }
    for cell in pattern.lanes.iter().flatten() {
        if cell.midi > 127
            || cell.velocity > 127
            || cell.probability > 100
            || !(0.0..=1.0).contains(&cell.gate_fraction)
        {
            return Err("Invalid note cell");
        }
        if !(-1..=6).contains(&cell.lock.machine)
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

pub fn save(
    path: &Path,
    bpm: f64,
    master_send: f32,
    patterns: &[Pattern],
    selected_pattern: usize,
    chain: &PatternChain,
    samples: &[Option<DecodedWav>; NUM_BANKS],
    custom_shader: Option<&CustomShader>,
) -> Result<(), String> {
    let project = ProjectFile {
        version: PROJECT_VERSION,
        bpm,
        master_send,
        patterns: patterns.to_vec(),
        selected_pattern,
        chain: chain.as_slice().to_vec(),
        sample_banks: core::array::from_fn(|bank| samples[bank].as_ref().map(SampleFile::from)),
        custom_shader: custom_shader.cloned(),
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
        patterns[1].lanes[4][63] = NoteCell {
            active: true,
            air: true,
            fill: FillCondition::WhenNoFill,
            probability: 37,
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
        save(&path, 120.0, 0.0, &patterns, 0, &chain, &samples, None).unwrap();
        let bpm = f64::from_bits(120.0_f64.to_bits() + 1);
        let send = 0.37_f32;
        save(&path, bpm, send, &patterns, 1, &chain, &samples, None.unwrap();
        let project = load(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(project.bpm.to_bits(), bpm.to_bits());
        assert_eq!(project.master_send.to_bits(), send.to_bits());
        assert_eq!(project.selected_pattern, 1);
        assert_eq!(project.chain, chain);
        for (original, loaded) in patterns.iter().zip(&project.patterns) {
            assert_eq!(original.rate, loaded.rate);
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
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: vec![1],
            sample_banks: core::array::from_fn(|_| None),
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
        pat.lanes[0][0].lock.machine = 7;
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.0,
            patterns: vec![pat],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
        custom_shader: None,
        };
        let json = serde_json::to_value(&project).unwrap();
        assert!(project_error(&json).contains("Invalid sound lock"));
    }

    #[test]
    fn older_projects_default_to_dry_and_invalid_send_is_rejected() {
        let project = ProjectFile {
            version: PROJECT_VERSION,
            bpm: 120.0,
            master_send: 0.5,
            patterns: vec![Pattern::default()],
            selected_pattern: 0,
            chain: vec![],
            sample_banks: core::array::from_fn(|_| None),
        custom_shader: None,
        };
        let mut json = serde_json::to_value(project).unwrap();
        json.as_object_mut().unwrap().remove("master_send");
        let old: ProjectFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(old.into_project().unwrap().master_send, 0.0);
        json["master_send"] = serde_json::json!(1.25);
        assert!(project_error(&json).contains("Master delay send"));
    }

    #[test]
    fn custom_shader_round_trips_and_legacy_projects_default_to_none() {
        let path = std::env::temp_dir()
            .join(format!("leviathan-shader-rt-{}.json", std::process::id()));
        let patterns = [Pattern::default()];
        let chain = PatternChain::default();
        let samples = core::array::from_fn(|_| None);
        let shader = CustomShader {
            name: "starter_custom.wgsl".to_string(),
            wgsl: leviathan_gpu::STARTER_SHADER_TEMPLATE_WGSL.to_string(),
        };

        save(
            &path,
            120.0,
            0.25,
            &patterns,
            0,
            &chain,
            &samples,
            Some(&shader),
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.custom_shader, Some(shader));

        let raw_bytes = fs::read(&path).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&raw_bytes).unwrap();
        json.as_object_mut().unwrap().remove("custom_shader");
        let legacy: ProjectFile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(legacy.into_project().unwrap().custom_shader, None);

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

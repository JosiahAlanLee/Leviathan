use crate::{MachineKind, MACHINE_COUNT};
use leviathan_core::{SoundLock, NUM_LANES, SOUND_PARAMS};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub default: f32,
}

impl ParamSpec {
    pub const fn new(name: &'static str, default: f32) -> Self {
        Self { name, default }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MachineSpec {
    pub kind: MachineKind,
    pub id: u8,
    pub slug: &'static str,
    pub badge: &'static str,
    pub name: &'static str,
    pub params: [ParamSpec; SOUND_PARAMS],
}

impl MachineSpec {
    #[inline]
    pub const fn default_params(&self) -> [f32; SOUND_PARAMS] {
        [
            self.params[0].default,
            self.params[1].default,
            self.params[2].default,
            self.params[3].default,
            self.params[4].default,
            self.params[5].default,
            self.params[6].default,
            self.params[7].default,
        ]
    }
}

pub const MACHINE_SPECS: [MachineSpec; MACHINE_COUNT] = [
    MachineSpec {
        kind: MachineKind::KickDrum,
        id: 0,
        slug: "kick",
        badge: "KICK",
        name: "Kick Drum",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("Drive", 0.35),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Resonance", 0.50),
            ParamSpec::new("Punch", 0.50),
            ParamSpec::new("Aux 5", 0.50),
            ParamSpec::new("Aux 6", 0.50),
            ParamSpec::new("Aux 7", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::FourOpFm,
        id: 1,
        slug: "fm4",
        badge: "FM4",
        name: "4-Op FM",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("FmIndex", 0.35),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Alg(0-3)", 0.28),
            ParamSpec::new("C:M Ratio", 0.45),
            ParamSpec::new("ModDecay", 0.28),
            ParamSpec::new("Op4 Fbk", 0.20),
            ParamSpec::new("Detune", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::SubBass,
        id: 2,
        slug: "sub",
        badge: "SUB",
        name: "Sub Bass",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("Drive", 0.50),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Resonance", 0.50),
            ParamSpec::new("Aux 4", 0.50),
            ParamSpec::new("Aux 5", 0.50),
            ParamSpec::new("Aux 6", 0.50),
            ParamSpec::new("Aux 7", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::NoiseWash,
        id: 3,
        slug: "air",
        badge: "AIR",
        name: "Noise Wash",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("Level", 0.50),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Resonance", 0.50),
            ParamSpec::new("Aux 4", 0.50),
            ParamSpec::new("Aux 5", 0.50),
            ParamSpec::new("Aux 6", 0.50),
            ParamSpec::new("Aux 7", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::VarSampler,
        id: 4,
        slug: "smpl",
        badge: "SMPL",
        name: "VarSampler",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("ChipClk", 0.42),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("WaveBank", 0.50),
            ParamSpec::new("LpStart", 0.00),
            ParamSpec::new("LpWindow", 0.85),
            ParamSpec::new("LpScan", 0.15),
            ParamSpec::new("Mode/Bit", 0.55),
        ],
    },
    MachineSpec {
        kind: MachineKind::AcidLead,
        id: 5,
        slug: "acid",
        badge: "ACID",
        name: "Acid Lead",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("Drive", 0.50),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Resonance", 0.50),
            ParamSpec::new("Aux 4", 0.50),
            ParamSpec::new("Aux 5", 0.50),
            ParamSpec::new("Aux 6", 0.50),
            ParamSpec::new("Aux 7", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::ChordStab,
        id: 6,
        slug: "chrd",
        badge: "CHRD",
        name: "Chord Stab",
        params: [
            ParamSpec::new("Decay", 0.35),
            ParamSpec::new("Spread", 0.50),
            ParamSpec::new("Cutoff", 0.50),
            ParamSpec::new("Resonance", 0.50),
            ParamSpec::new("Aux 4", 0.50),
            ParamSpec::new("Aux 5", 0.50),
            ParamSpec::new("Aux 6", 0.50),
            ParamSpec::new("Aux 7", 0.50),
        ],
    },
    MachineSpec {
        kind: MachineKind::CzPhaseDist,
        id: 7,
        slug: "czpd",
        badge: "CZPD",
        name: "CZ Phase Distortion",
        params: [
            ParamSpec::new("Wave A", 0.06),
            ParamSpec::new("Wave B / Mix", 0.38),
            ParamSpec::new("DCW Depth", 0.72),
            ParamSpec::new("DCW Decay", 0.34),
            ParamSpec::new("Amp Attack", 0.08),
            ParamSpec::new("Amp Release", 0.28),
            ParamSpec::new("Ring / Noise", 0.0),
            ParamSpec::new("Detune", 0.55),
        ],
    },
    MachineSpec {
        kind: MachineKind::PolivoksSub,
        id: 8,
        slug: "pvx2",
        badge: "PVX2",
        name: "Polivoks Subtractive",
        params: [
            ParamSpec::new("Osc Wave / Mix", 0.34),
            ParamSpec::new("Detune / Interval", 0.53),
            ParamSpec::new("Cross-FM / Sync", 0.21),
            ParamSpec::new("Cutoff", 0.37),
            ParamSpec::new("Resonance", 0.62),
            ParamSpec::new("LP/BP & Drive", 0.25),
            ParamSpec::new("Env Mod", 0.65),
            ParamSpec::new("Decay", 0.36),
        ],
    },
    MachineSpec {
        kind: MachineKind::VastFunSynth,
        id: 9,
        slug: "vast",
        badge: "VAST",
        name: "VAST FUN Synth",
        params: [
            ParamSpec::new("Algorithm", 0.12),
            ParamSpec::new("Warp / Drive", 0.32),
            ParamSpec::new("FUN Equation", 0.1),
            ParamSpec::new("FUN Depth", 0.45),
            ParamSpec::new("LFO Rate", 0.28),
            ParamSpec::new("Cutoff", 0.56),
            ParamSpec::new("Resonance", 0.33),
            ParamSpec::new("Amp Decay", 0.48),
        ],
    },
    MachineSpec {
        kind: MachineKind::ElectroPerc,
        id: 10,
        slug: "perc",
        badge: "PERC",
        name: "Electro Percussion",
        params: [
            ParamSpec::new("Decay", 0.32),
            ParamSpec::new("Mode", 0.10),
            ParamSpec::new("BodyTune", 0.45),
            ParamSpec::new("Snappy", 0.55),
            ParamSpec::new("MetalSpread", 0.50),
            ParamSpec::new("BandTone", 0.58),
            ParamSpec::new("Drive", 0.30),
            ParamSpec::new("PitchEnv", 0.40),
        ],
    },
    MachineSpec {
        kind: MachineKind::WestCoastLpg,
        id: 11,
        slug: "west",
        badge: "WEST",
        name: "West Coast LPG",
        params: [
            ParamSpec::new("VactrolDecay", 0.40),
            ParamSpec::new("FoldDrive", 0.35),
            ParamSpec::new("Symmetry", 0.50),
            ParamSpec::new("TzfmDepth", 0.25),
            ParamSpec::new("ModRatio", 0.50),
            ParamSpec::new("RingMix", 0.00),
            ParamSpec::new("LpgMode", 0.50),
            ParamSpec::new("Strike", 0.60),
        ],
    },
    MachineSpec {
        kind: MachineKind::OtisTranswave,
        id: 12,
        slug: "otis",
        badge: "OTIS",
        name: "OTIS Transwave",
        params: [
            ParamSpec::new("Decay", 0.48),
            ParamSpec::new("TableSel", 0.00),
            ParamSpec::new("FramePos", 0.20),
            ParamSpec::new("ScanDepth", 0.55),
            ParamSpec::new("K1Cutoff", 0.65),
            ParamSpec::new("K2Reso", 0.30),
            ParamSpec::new("FixedGrit", 0.25),
            ParamSpec::new("LoopMode", 0.50),
        ],
    },
];

#[inline]
pub const fn machine_spec(kind: MachineKind) -> &'static MachineSpec {
    &MACHINE_SPECS[kind as usize]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LanePatch {
    pub machine: MachineKind,
    pub params: [f32; SOUND_PARAMS],
}

impl LanePatch {
    #[inline]
    pub const fn for_machine(machine: MachineKind) -> Self {
        Self {
            machine,
            params: machine_spec(machine).default_params(),
        }
    }

    #[inline]
    pub fn is_valid(&self) -> bool {
        let mut i = 0;
        while i < SOUND_PARAMS {
            let v = self.params[i];
            if !v.is_finite() || v < 0.0 || v > 1.0 {
                return false;
            }
            i += 1;
        }
        true
    }

    #[inline]
    pub fn sanitized(mut self) -> Self {
        let defaults = machine_spec(self.machine).default_params();
        let mut i = 0;
        while i < SOUND_PARAMS {
            let v = self.params[i];
            self.params[i] = if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                defaults[i]
            };
            i += 1;
        }
        self
    }

    #[inline]
    pub const fn resolve_lock(&self, step_lock: &SoundLock) -> SoundLock {
        step_lock.resolved_with_base(self.machine as i16, &self.params)
    }
}

impl Default for LanePatch {
    fn default() -> Self {
        Self::for_machine(MachineKind::KickDrum)
    }
}

pub const DEFAULT_LANE_MACHINES: [MachineKind; NUM_LANES] = [
    MachineKind::KickDrum,
    MachineKind::FourOpFm,
    MachineKind::SubBass,
    MachineKind::NoiseWash,
    MachineKind::VarSampler,
    MachineKind::AcidLead,
];

pub const DEFAULT_LANE_PATCHES: [LanePatch; NUM_LANES] = [
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[0]),
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[1]),
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[2]),
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[3]),
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[4]),
    LanePatch::for_machine(DEFAULT_LANE_MACHINES[5]),
];

#[inline]
pub const fn default_lane_patches() -> [LanePatch; NUM_LANES] {
    DEFAULT_LANE_PATCHES
}

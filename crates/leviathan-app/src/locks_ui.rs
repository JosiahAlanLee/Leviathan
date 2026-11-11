use eframe::egui;
use leviathan_core::{NoteCell, Pattern, SoundLock};
use leviathan_dsp::MachineKind;

const ALL_MACHINES: [MachineKind; 7] = [
    MachineKind::KickDrum,
    MachineKind::FourOpFm,
    MachineKind::SubBass,
    MachineKind::NoiseWash,
    MachineKind::VarSampler,
    MachineKind::AcidLead,
    MachineKind::ChordStab,
];

pub fn apply_starter_sound_locks(pat: &mut Pattern) {
    for row in 0..64 {
        let step16 = row % 16;
        let bar = row / 16;

        if matches!(step16, 4 | 12) {
            pat.lanes[1][row].lock = SoundLock {
                machine: MachineKind::FourOpFm as i16,
                mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 5),
                values: [0.35, 0.46, 0.65, 0.30, 0.45, 0.22, 0.15, 0.10],
            };
        } else if matches!(step16, 9 | 15) && bar % 2 == 1 {
            pat.lanes[1][row] = NoteCell {
                active: true,
                midi: 73,
                velocity: 96,
                gate_fraction: 0.40,
                lock: SoundLock {
                    machine: MachineKind::FourOpFm as i16,
                    mask: (1 << 1) | (1 << 3) | (1 << 4) | (1 << 6),
                    values: [0.45, 0.82, 0.75, 0.05, 0.55, 0.35, 0.68, 0.20],
                },
                ..NoteCell::default()
            };
        }

        if step16 == 0 && bar % 2 == 0 {
            pat.lanes[4][row].lock = SoundLock {
                machine: MachineKind::VarSampler as i16,
                mask: (1 << 0) | (1 << 1) | (1 << 3) | (1 << 5) | (1 << 6) | (1 << 7),
                values: [0.75, 0.62, 0.72, 0.60, 0.05, 0.55, 0.45, 0.85],
            };
        } else if step16 == 8 && bar % 2 == 1 {
            pat.lanes[4][row] = NoteCell {
                active: true,
                midi: 49,
                velocity: 116,
                gate_fraction: 0.50,
                lock: SoundLock {
                    machine: MachineKind::VarSampler as i16,
                    mask: (1 << 0) | (1 << 1) | (1 << 3) | (1 << 7),
                    values: [0.50, 0.12, 0.58, 0.05, 0.0, 0.90, 0.0, 0.15],
                },
                ..NoteCell::default()
            };
        }
    }
}

pub fn param_labels(machine: MachineKind) -> [&'static str; 8] {
    match machine {
        MachineKind::VarSampler => [
            "P0 Decay",
            "P1 ChipClk",
            "P2 Cutoff",
            "P3 WaveBank",
            "P4 LpStart",
            "P5 LpWindow",
            "P6 LpScan",
            "P7 Mode/Bit",
        ],
        MachineKind::FourOpFm => [
            "P0 Decay",
            "P1 FmIndex",
            "P2 Cutoff",
            "P3 Alg(0-3)",
            "P4 C:M Ratio",
            "P5 ModDecay",
            "P6 Op4 Fbk",
            "P7 Detune",
        ],
        _ => [
            "P0 Decay",
            "P1 Drive",
            "P2 Cutoff",
            "P3 Aux3",
            "P4 Aux4",
            "P5 Aux5",
            "P6 Aux6",
            "P7 Aux7",
        ],
    }
}

pub fn draw_step_lock_inspector(
    ui: &mut egui::Ui,
    lane: usize,
    row: usize,
    lane_name: &str,
    cell: &mut NoteCell,
) -> bool {
    let mut dirty = false;
    let eff = MachineKind::from_lane_and_lock(lane, &cell.lock);
    ui.separator();
    ui.heading(format!("Step Lock — {lane_name} Row {row:02X} ({})", eff.short_name()));
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(cell.lock.machine < 0, "Lane").clicked() {
            cell.lock.machine = -1;
            dirty = true;
        }
        for m in ALL_MACHINES {
            let code = m as i16;
            if ui.selectable_label(cell.lock.machine == code, m.short_name()).clicked() {
                cell.lock.machine = code;
                dirty = true;
            }
        }
    });
    for (slot, &name) in param_labels(eff).iter().enumerate() {
        ui.horizontal(|ui| {
            let mut locked = ((cell.lock.mask >> slot) & 1) != 0;
            if ui.checkbox(&mut locked, "").changed() {
                if locked {
                    cell.lock.mask |= 1 << slot;
                } else {
                    cell.lock.mask &= !(1 << slot);
                }
                dirty = true;
            }
            if ui
                .add(egui::Slider::new(&mut cell.lock.values[slot], 0.0..=1.0).text(name))
                .changed()
            {
                cell.lock.mask |= 1 << slot;
                dirty = true;
            }
        });
    }
    dirty
}

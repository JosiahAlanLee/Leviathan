use eframe::egui;
use leviathan_core::{NoteCell, Pattern, SoundLock, SOUND_PARAMS};
use leviathan_dsp::{LanePatch, MachineKind, MACHINE_SPECS};

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

pub fn draw_lane_patch_inspector(ui: &mut egui::Ui, lane: usize, patch: &mut LanePatch) -> bool {
    let mut dirty = false;
    egui::CollapsingHeader::new(format!("Lane {} Patch ({})", lane + 1, patch.machine.display_name()))
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for spec in &MACHINE_SPECS {
                    if ui
                        .selectable_label(patch.machine == spec.kind, spec.badge)
                        .on_hover_text(spec.name)
                        .clicked()
                        && patch.machine != spec.kind
                    {
                        *patch = LanePatch::for_machine(spec.kind);
                        dirty = true;
                    }
                }
                if ui
                    .small_button("Reset")
                    .on_hover_text("Restore default macro knobs for this machine")
                    .clicked()
                {
                    *patch = LanePatch::for_machine(patch.machine);
                    dirty = true;
                }
            });
            let spec = patch.machine.spec();
            for slot in 0..SOUND_PARAMS {
                if ui
                    .add(egui::Slider::new(&mut patch.params[slot], 0.0..=1.0).text(format!(
                        "P{slot} {}",
                        spec.params[slot].name
                    )))
                    .changed()
                {
                    dirty = true;
                }
            }
        });
    dirty
}


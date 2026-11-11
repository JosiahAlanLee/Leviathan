use eframe::egui;
use leviathan_core::{
    AutomationRow, FillCondition, NoteCell, PlaybackRate, SoundLock, NUM_LANES, ROWS_PER_PATTERN,
};
use leviathan_dsp::MachineKind;

pub const LANE_NAMES: [&str; NUM_LANES] = ["0 KICK", "1 FM4", "2 SUB", "3 AIR", "4 SMPL", "5 ACID"];

const ALL_MACHINES: [MachineKind; 7] = [
    MachineKind::KickDrum,
    MachineKind::FourOpFm,
    MachineKind::SubBass,
    MachineKind::NoiseWash,
    MachineKind::VarSampler,
    MachineKind::AcidLead,
    MachineKind::ChordStab,
];

const ALL_RATES: [(PlaybackRate, &str); 6] = [
    (PlaybackRate::Quarter, "1/4x"),
    (PlaybackRate::Half, "1/2x"),
    (PlaybackRate::OneX, "1x"),
    (PlaybackRate::TwoX, "2x"),
    (PlaybackRate::FourX, "4x"),
    (PlaybackRate::EightX, "8x"),
];

const ALL_FILLS: [(FillCondition, &str, &str); 3] = [
    (
        FillCondition::Always,
        "Always",
        "Trigger regardless of FILL state",
    ),
    (
        FillCondition::WhenFill,
        "FILL",
        "Trigger only while FILL is active",
    ),
    (
        FillCondition::WhenNoFill,
        "!FILL",
        "Mute step while FILL is active",
    ),
];

fn param_labels(machine: MachineKind) -> [&'static str; 8] {
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

/// Returns `*` for sound-locked steps, `?` for conditional (`Prob` < 100 or `FILL`) steps, or `' '`.
#[inline]
pub fn step_marker(cell: &NoteCell) -> char {
    if cell.lock.mask != 0 || cell.lock.machine >= 0 {
        '*'
    } else if cell.probability < 100 || cell.fill != FillCondition::Always {
        '?'
    } else {
        ' '
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InspectorEdits {
    pub cell: Option<NoteCell>,
    pub automation: Option<AutomationRow>,
    pub pattern_length: Option<usize>,
    pub pattern_rate: Option<PlaybackRate>,
}

pub fn draw(
    ui: &mut egui::Ui,
    lane: usize,
    row: usize,
    current_cell: NoteCell,
    current_auto: AutomationRow,
    current_length: usize,
    current_rate: PlaybackRate,
) -> InspectorEdits {
    let mut edits = InspectorEdits::default();
    let safe_lane = lane.min(NUM_LANES - 1);
    let safe_row = row.min(ROWS_PER_PATTERN - 1);

    ui.heading("Pattern Timing");
    ui.horizontal_wrapped(|ui| {
        ui.label("Rows");
        let mut len = current_length.clamp(1, ROWS_PER_PATTERN);
        if ui
            .add(egui::DragValue::new(&mut len).range(1..=ROWS_PER_PATTERN))
            .on_hover_text("Active pattern step count (1..=64)")
            .changed()
        {
            edits.pattern_length = Some(len);
        }
        ui.separator();
        for (rate, label) in ALL_RATES {
            if ui.selectable_label(current_rate == rate, label).clicked() && rate != current_rate {
                edits.pattern_rate = Some(rate);
            }
        }
    });

    ui.separator();

    let mut edit_cell = current_cell;
    let mut cell_dirty = false;
    let eff_machine = MachineKind::from_lane_and_lock(safe_lane, &edit_cell.lock);

    ui.heading(format!(
        "Step Lock -- {} Row {:02X} ({})",
        LANE_NAMES[safe_lane],
        safe_row,
        eff_machine.short_name()
    ));
    ui.small("Shift+Arrows/Click select; Ctrl+Z/Y undo/redo; Ctrl+C/X/V; Ctrl+Up/Dn transpose.");

    ui.horizontal(|ui| {
        if ui.checkbox(&mut edit_cell.active, "On").changed() {
            cell_dirty = true;
        }
        if ui.checkbox(&mut edit_cell.air, "Air").changed() {
            cell_dirty = true;
        }
        ui.label("MIDI");
        if ui
            .add(egui::DragValue::new(&mut edit_cell.midi).range(12..=108))
            .changed()
        {
            cell_dirty = true;
        }
        ui.label("Vel");
        if ui
            .add(egui::DragValue::new(&mut edit_cell.velocity).range(1..=127))
            .changed()
        {
            cell_dirty = true;
        }
    });

    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Slider::new(&mut edit_cell.probability, 0..=100)
                    .suffix("%")
                    .text("Prob"),
            )
            .on_hover_text("Deterministic lowbias32 trigger probability (0..=100%)")
            .changed()
        {
            cell_dirty = true;
        }
    });

    ui.horizontal(|ui| {
        if ui
            .add(egui::Slider::new(&mut edit_cell.gate_fraction, 0.0..=1.0).text("Gate"))
            .on_hover_text("Step gate duty cycle fraction (0.0..=1.0)")
            .changed()
        {
            cell_dirty = true;
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label("Fill:");
        for (cond, label, tip) in ALL_FILLS {
            if ui
                .selectable_label(edit_cell.fill == cond, label)
                .on_hover_text(tip)
                .clicked()
                && edit_cell.fill != cond
            {
                edit_cell.fill = cond;
                cell_dirty = true;
            }
        }
        let has_lock = edit_cell.lock.machine >= 0 || edit_cell.lock.mask != 0;
        if ui
            .add_enabled(has_lock, egui::Button::new("Clear Locks"))
            .on_hover_text("Reset machine and parameter locks on this step")
            .clicked()
        {
            edit_cell.lock = SoundLock::default();
            cell_dirty = true;
        }
    });

    ui.horizontal_wrapped(|ui| {
        if ui
            .selectable_label(edit_cell.lock.machine < 0, "Lane")
            .clicked()
            && edit_cell.lock.machine != -1
        {
            edit_cell.lock.machine = -1;
            cell_dirty = true;
        }
        for m in ALL_MACHINES {
            let code = m as i16;
            if ui
                .selectable_label(edit_cell.lock.machine == code, m.short_name())
                .clicked()
                && edit_cell.lock.machine != code
            {
                edit_cell.lock.machine = code;
                cell_dirty = true;
            }
        }
    });

    let labels = param_labels(eff_machine);
    for (slot, &name) in labels.iter().enumerate() {
        ui.horizontal(|ui| {
            let mut locked = ((edit_cell.lock.mask >> slot) & 1) != 0;
            if ui.checkbox(&mut locked, "").changed() {
                if locked {
                    edit_cell.lock.mask |= 1 << slot;
                } else {
                    edit_cell.lock.mask &= !(1 << slot);
                }
                cell_dirty = true;
            }
            if ui
                .add(egui::Slider::new(&mut edit_cell.lock.values[slot], 0.0..=1.0).text(name))
                .changed()
            {
                edit_cell.lock.mask |= 1 << slot;
                cell_dirty = true;
            }
        });
    }

    if cell_dirty {
        edits.cell = Some(edit_cell);
    }

    ui.separator();

    let mut edit_auto = current_auto;
    let mut auto_dirty = false;

    ui.heading(format!("Visual Automation -- Row {:02X}", safe_row));
    if ui
        .add(egui::Slider::new(&mut edit_auto.z, 0.0..=1.0).text("Z Zoom"))
        .on_hover_text("Shader zoom CV (0..=10V normalized)")
        .changed()
    {
        auto_dirty = true;
    }
    if ui
        .add(egui::Slider::new(&mut edit_auto.roll, -1.0..=1.0).text("Roll"))
        .on_hover_text("Shader rotation CV (-5V..=+5V normalized)")
        .changed()
    {
        auto_dirty = true;
    }
    if ui
        .add(egui::Slider::new(&mut edit_auto.feedback, 0.0..=1.0).text("Feedback"))
        .on_hover_text("Ping-pong temporal decay CV (0..=10V normalized)")
        .changed()
    {
        auto_dirty = true;
    }

    ui.horizontal_wrapped(|ui| {
        ui.label("Byte");
        if ui
            .add(egui::DragValue::new(&mut edit_auto.byte_word).range(0..=255))
            .on_hover_text("8-bit ByteBus word emitted on this row (0..=255)")
            .changed()
        {
            auto_dirty = true;
        }
        ui.monospace(format!("0x{:02X}", edit_auto.byte_word));
        for bit in 0..8u8 {
            let high = ((edit_auto.byte_word >> bit) & 1) != 0;
            if ui
                .selectable_label(high, format!("{bit}"))
                .on_hover_text(format!("Toggle ByteBus bit B{bit}"))
                .clicked()
            {
                edit_auto.byte_word ^= 1 << bit;
                auto_dirty = true;
            }
        }
    });

    if auto_dirty {
        edits.automation = Some(edit_auto);
    }

    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_marker_distinguishes_locks_and_conditions() {
        let mut cell = NoteCell::default();
        assert_eq!(step_marker(&cell), ' ');

        cell.probability = 65;
        assert_eq!(step_marker(&cell), '?');

        cell.probability = 100;
        cell.fill = FillCondition::WhenFill;
        assert_eq!(step_marker(&cell), '?');

        cell.lock.mask = 1;
        assert_eq!(step_marker(&cell), '*');
    }

    #[test]
    fn inspector_renders_headlessly_without_spurious_edits() {
        let ctx = egui::Context::default();
        let mut captured = InspectorEdits::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                captured = draw(
                    ui,
                    1,
                    4,
                    NoteCell::default(),
                    AutomationRow::default(),
                    64,
                    PlaybackRate::OneX,
                );
            });
        });
        assert_eq!(captured, InspectorEdits::default());
    }
}

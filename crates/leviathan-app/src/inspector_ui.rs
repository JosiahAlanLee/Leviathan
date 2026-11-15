use eframe::egui;
use leviathan_core::{
    AutomationRow, FillCondition, NoteCell, SoundLock, MAX_NUDGE, MAX_RETRIG, NUM_LANES,
};
use leviathan_dsp::{LanePatch, MachineKind, MACHINE_SPECS};

use crate::param_ui;

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

pub fn draw_step(
    ui: &mut egui::Ui,
    lane: usize,
    row: usize,
    current_cell: NoteCell,
    lane_patch: &LanePatch,
    kit_curve: leviathan_dsp::otis_kit::VelocityCurve,
) -> Option<NoteCell> {
    let mut edit_cell = current_cell;
    let mut cell_dirty = false;
    let eff_machine = MachineKind::from_patch_and_lock(lane_patch, &edit_cell.lock);

    ui.heading(format!(
        "Lane {} · Row {row:02X} · {}",
        lane.min(NUM_LANES - 1) + 1,
        eff_machine.short_name(),
    ));
    if edit_cell.lock.machine >= 0 {
        ui.small(format!(
            "Step machine override · Lane patch {}",
            lane_patch.machine.short_name()
        ));
    }

    ui.horizontal(|ui| {
        if ui.checkbox(&mut edit_cell.active, "On").changed() {
            cell_dirty = true;
        }
        if ui.checkbox(&mut edit_cell.air, "Air").changed() {
            cell_dirty = true;
        }
        ui.label("MIDI");
        if ui
            .add(egui::DragValue::new(&mut edit_cell.midi).range(0..=127))
            .changed()
        {
            cell_dirty = true;
        }
        ui.label("Vel");
        if ui
            .add(egui::DragValue::new(&mut edit_cell.velocity).range(0..=127))
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
            .on_hover_text("Gate length within each trigger interval")
            .changed()
        {
            cell_dirty = true;
        }
    });

    ui.vertical(|ui| {
        cell_dirty |= ui
            .add(
                egui::Slider::new(&mut edit_cell.micro_offset, -MAX_NUDGE..=MAX_NUDGE)
                    .suffix(" ticks")
                    .text("Nudge"),
            )
            .on_hover_text("Move this hit earlier or later. One row is 48 ticks.")
            .changed();
        cell_dirty |= ui
            .add(egui::Slider::new(&mut edit_cell.retrig, 1..=MAX_RETRIG).text("Retrig"))
            .on_hover_text("Number of evenly spaced hits in this step")
            .changed();
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

    let previous = edit_cell.lock.machine;
    let machine_label = if previous < 0 {
        "Inherit lane"
    } else {
        eff_machine.display_name()
    };
    egui::ComboBox::from_id_salt("step_machine")
        .selected_text(machine_label)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut edit_cell.lock.machine, -1, "Inherit lane");
            for spec in &MACHINE_SPECS {
                ui.selectable_value(&mut edit_cell.lock.machine, i16::from(spec.id), spec.name);
            }
        });
    cell_dirty |= previous != edit_cell.lock.machine;

    let spec = MachineKind::from_patch_and_lock(lane_patch, &edit_cell.lock).spec();
    if spec.kind == MachineKind::OtisKit {
        cell_dirty |= ui
            .push_id((lane, row), |ui| {
                param_ui::draw_kit_velocity(ui, &mut edit_cell.velocity, kit_curve)
            })
            .inner;
    }
    let base_params = lane_patch.step_base_params(spec.kind);
    for (slot, &base) in base_params.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            let mut locked = edit_cell.lock.has_param(slot);
            let tip = if spec.kind == MachineKind::OtisTwin && slot == param_ui::MODE_GRIT_SLOT {
                "Scan mode and grit share this step lock. Clear it to inherit both."
            } else {
                "Override this parameter on this step"
            };
            if ui
                .checkbox(&mut locked, "")
                .on_hover_text(tip)
                .changed()
            {
                if locked {
                    edit_cell.lock.mask |= 1 << slot;
                    edit_cell.lock.values[slot] = base;
                } else {
                    edit_cell.lock.mask &= !(1 << slot);
                }
                cell_dirty = true;
            }
            let mut value = if locked {
                edit_cell.lock.values[slot]
            } else {
                base
            };
            if param_ui::draw_macro(ui, spec, slot, &mut value, Some(base)) {
                edit_cell.lock.mask |= 1 << slot;
                edit_cell.lock.values[slot] = value;
                cell_dirty = true;
            }
        });
    }
    cell_dirty.then_some(edit_cell)
}

pub fn draw_automation(
    ui: &mut egui::Ui,
    row: usize,
    current_auto: AutomationRow,
) -> Option<AutomationRow> {
    let mut edit_auto = current_auto;
    let mut auto_dirty = false;

    ui.heading(format!("Row {row:02X} automation"));
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

    let mut fixed_byte = edit_auto.fixed_byte().is_some();
    if ui
        .checkbox(&mut fixed_byte, "Fixed ByteBus word")
        .on_hover_text("Use the byte below, including 0x00. When off, use the live lane gate mask.")
        .changed()
    {
        edit_auto.byte_override = fixed_byte;
        if !fixed_byte {
            edit_auto.byte_word = 0;
        }
        auto_dirty = true;
    }
    ui.add_enabled_ui(fixed_byte, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Byte");
            if ui
                .add(egui::DragValue::new(&mut edit_auto.byte_word).range(0..=255))
                .changed()
            {
                edit_auto.byte_override = true;
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
                    edit_auto.byte_override = true;
                    auto_dirty = true;
                }
            }
        });
    });

    auto_dirty.then_some(edit_auto)
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
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let cell = NoteCell {
                    midi: 1,
                    velocity: 0,
                    ..NoteCell::default()
                };
                assert!(draw_step(ui, 1, 4, cell, &LanePatch::default(), Default::default()).is_none());
                let mut patch = LanePatch::for_machine(MachineKind::OtisTwin);
                patch.params[param_ui::MODE_GRIT_SLOT] = 0.28;
                let saved = patch;
                assert!(!crate::locks_ui::draw_lane_patch_inspector(ui, 2, &mut patch, false));
                assert_eq!(patch, saved);
                assert!(draw_step(ui, 1, 4, cell, &patch, Default::default()).is_none());
                let mut locked = cell;
                locked.lock.machine = MachineKind::OtisTwin as i16;
                locked.lock.mask = 1 << param_ui::MODE_GRIT_SLOT;
                locked.lock.values[param_ui::MODE_GRIT_SLOT] = 0.669_999_96;
                assert!(draw_step(ui, 2, 5, locked, &LanePatch::default(), Default::default()).is_none());
                locked.lock.machine = MachineKind::OtisKit as i16;
                locked.velocity = 110;
                assert!(draw_step(ui, 2, 6, locked, &LanePatch::default(), Default::default()).is_none());
                assert!(draw_automation(ui, 4, AutomationRow::default()).is_none());
            });
        });
    }
}

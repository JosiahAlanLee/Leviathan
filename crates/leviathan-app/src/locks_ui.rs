use eframe::egui;
use leviathan_core::SOUND_PARAMS;
use leviathan_dsp::{LanePatch, MACHINE_SPECS};

pub fn draw_lane_patch_inspector(
    ui: &mut egui::Ui,
    lane: usize,
    patch: &mut LanePatch,
    calibrated: bool,
) -> bool {
    let defaults = |machine| {
        if calibrated {
            LanePatch::for_machine_with_calibration(machine)
        } else {
            LanePatch::for_machine(machine)
        }
    };
    let mut dirty = false;
    ui.heading(format!(
        "Lane {} · {}",
        lane + 1,
        patch.machine.short_name()
    ));
    ui.horizontal_wrapped(|ui| {
        let mut machine = patch.machine;
        egui::ComboBox::from_id_salt("lane_machine")
            .selected_text(machine.display_name())
            .show_ui(ui, |ui| {
                for spec in &MACHINE_SPECS {
                    ui.selectable_value(&mut machine, spec.kind, spec.name);
                }
            });
        let reset = ui
            .button("Reset patch")
            .on_hover_text("Restore default macro knobs for this machine")
            .clicked();
        if machine != patch.machine || reset {
            *patch = defaults(machine);
            dirty = true;
        }
    });
    let spec = patch.machine.spec();
    for slot in 0..SOUND_PARAMS {
        dirty |= ui
            .add(
                egui::Slider::new(&mut patch.params[slot], 0.0..=1.0)
                    .text(format!("P{slot} {}", spec.params[slot].name)),
            )
            .changed();
    }
    dirty
}

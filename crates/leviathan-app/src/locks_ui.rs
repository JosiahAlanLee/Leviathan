use eframe::egui;
use leviathan_core::SOUND_PARAMS;
use leviathan_dsp::{LanePatch, MACHINE_SPECS};

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


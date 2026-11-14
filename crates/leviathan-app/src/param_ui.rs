use eframe::egui;
use leviathan_dsp::{otis_kit, MachineKind, MachineSpec, TwinLoopMode};

pub const MODE_GRIT_SLOT: usize = 7;
const KIT_GAIN_HELP: &str =
    "Relative hit gain at the stored MIDI velocity, before slot level and mixing.";

pub fn draw_macro(
    ui: &mut egui::Ui,
    machine: &MachineSpec,
    slot: usize,
    value: &mut f32,
    inherited: Option<f32>,
) -> bool {
    if machine.kind == MachineKind::OtisTwin && slot == MODE_GRIT_SLOT {
        return draw_mode_grit(ui, value, inherited);
    }

    let mut slider =
        egui::Slider::new(value, 0.0..=1.0).text(format!("P{slot} {}", machine.params[slot].name));
    if inherited.is_some() {
        slider = slider.clamping(egui::SliderClamping::Edits);
    }
    let response = ui.add(slider);
    if let Some(base) = inherited {
        response
            .on_hover_text(format!("Unlocked value · {base:.3}"))
            .changed()
    } else {
        response.changed()
    }
}

fn scan_label(mode: TwinLoopMode) -> &'static str {
    match mode {
        TwinLoopMode::OneShot => "One shot",
        TwinLoopMode::Bidir => "Bidirectional",
        TwinLoopMode::FwdWrap => "Forward loop",
    }
}

fn draw_mode_grit(ui: &mut egui::Ui, value: &mut f32, inherited: Option<f32>) -> bool {
    let (original_mode, grit) = TwinLoopMode::decode(*value);
    let mut mode = original_mode;
    let original_percent = grit * 100.0;
    let mut percent = original_percent;
    let response = ui.horizontal_wrapped(|ui| {
        ui.label("Scan mode");
        egui::ComboBox::from_id_salt("otis_twin_scan_mode")
            .selected_text(scan_label(mode))
            .width(100.0)
            .show_ui(ui, |ui| {
                for item in [
                    TwinLoopMode::OneShot,
                    TwinLoopMode::Bidir,
                    TwinLoopMode::FwdWrap,
                ] {
                    ui.selectable_value(&mut mode, item, scan_label(item));
                }
            });
        ui.label("Grit");
        ui.add(
            egui::DragValue::new(&mut percent)
                .range(0.0..=100.0)
                .speed(0.5)
                .fixed_decimals(2)
                .suffix("%")
                .update_while_editing(false),
        )
        .on_hover_text("Reduces the oscillator's update rate and resolution.");
        if percent >= 50.0 {
            ui.weak("Strong lo-fi coloration");
        }
    });
    if let Some(base) = inherited {
        response.response.on_hover_ui(|ui| {
            let (mode, grit) = TwinLoopMode::decode(base);
            ui.label(format!(
                "Unlocked setting · {} · {:.2}% grit",
                scan_label(mode),
                grit * 100.0
            ));
        });
    }
    // Opening and dismissing the number editor can round the displayed value.
    let grit_edited = (percent * 100.0).round() != (original_percent * 100.0).round();
    if mode == original_mode && !grit_edited {
        return false;
    }
    let encoded = mode.encode(if grit_edited { percent / 100.0 } else { grit });
    let changed = *value != encoded;
    *value = encoded;
    changed
}

fn kit_velocity_db(velocity: u8) -> Option<f32> {
    otis_kit::velocity_gain_db(f32::from(velocity.min(127)) / 127.0)
}

pub fn format_kit_velocity(velocity: u8) -> String {
    match kit_velocity_db(velocity) {
        None => "Silent".into(),
        Some(db) if db == 0.0 => "0 dB".into(),
        Some(db) => format!("{db:.2} dB"),
    }
}

fn parse_kit_velocity(input: &str) -> Option<u8> {
    let input = input.trim().to_ascii_lowercase().replace('−', "-");
    let value = input.strip_suffix("db").unwrap_or(&input).trim_end();
    if matches!(value, "silent" | "-inf" | "-infinity" | "-∞") {
        return Some(0);
    }
    let db = value.parse::<f32>().ok().filter(|db| db.is_finite())?;
    let db = db.clamp(kit_velocity_db(1).unwrap(), 0.0);
    (1..=127).min_by(|&left, &right| {
        let error = |velocity| (kit_velocity_db(velocity).unwrap() - db).abs();
        error(left).total_cmp(&error(right))
    })
}

pub fn draw_kit_velocity(ui: &mut egui::Ui, velocity: &mut u8) -> bool {
    let original = *velocity;
    ui.horizontal_wrapped(|ui| {
        ui.label("Kit velocity gain").on_hover_text(KIT_GAIN_HELP);
        ui.add(
            egui::DragValue::new(velocity)
                .range(0..=127)
                .custom_formatter(|value, _| format_kit_velocity(value as u8))
                .custom_parser(|input| parse_kit_velocity(input).map(f64::from))
                .update_while_editing(false),
        )
        .on_hover_text(KIT_GAIN_HELP);
    });
    *velocity != original
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_midi_velocity_round_trips_through_the_display() {
        for velocity in 0..=127 {
            assert_eq!(
                parse_kit_velocity(&format_kit_velocity(velocity)),
                Some(velocity)
            );
        }
        assert_eq!(parse_kit_velocity("−18.75 dB"), Some(94));
        assert_eq!(parse_kit_velocity("-9.8"), Some(110));
        assert_eq!(parse_kit_velocity("1e20"), Some(127));
        assert_eq!(parse_kit_velocity("-1e20"), Some(1));
        assert_eq!(parse_kit_velocity("-∞ dB"), Some(0));
        assert_eq!(parse_kit_velocity("NaN"), None);
    }
}

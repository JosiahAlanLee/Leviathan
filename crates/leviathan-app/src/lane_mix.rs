use eframe::egui;
use leviathan_dsp::ALL_LANES_AUDIBLE;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneMix {
    #[serde(default)]
    pub mute_mask: u8,
    #[serde(default)]
    pub solo_mask: u8,
}

#[derive(Clone, Copy)]
pub enum LaneMixAction {
    Mute(usize),
    Solo(usize),
    Clear,
}

impl LaneMix {
    pub fn audible_mask(self) -> u8 {
        let solo = self.solo_mask & ALL_LANES_AUDIBLE;
        let selected = if solo == 0 { ALL_LANES_AUDIBLE } else { solo };
        selected & !self.mute_mask
    }

    pub fn is_valid(self) -> bool {
        (self.mute_mask | self.solo_mask) & !ALL_LANES_AUDIBLE == 0
    }

    pub fn apply(&mut self, action: LaneMixAction) {
        match action {
            LaneMixAction::Mute(lane) => self.mute_mask ^= 1 << lane,
            LaneMixAction::Solo(lane) => self.solo_mask ^= 1 << lane,
            LaneMixAction::Clear => *self = Self::default(),
        }
    }

    pub fn header(&mut self, ui: &mut egui::Ui, name: &str, lane: usize) {
        let bit = 1 << lane;
        ui.vertical(|ui| {
            ui.monospace(name);
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.mute_mask & bit != 0, "M")
                    .on_hover_text(format!("Mute lane (Alt+{})", lane + 1))
                    .clicked()
                {
                    self.apply(LaneMixAction::Mute(lane));
                }
                if ui
                    .selectable_label(self.solo_mask & bit != 0, "S")
                    .on_hover_text(format!("Solo lane (Alt+Shift+{})", lane + 1))
                    .clicked()
                {
                    self.apply(LaneMixAction::Solo(lane));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mute_excludes_a_lane_even_when_soloed() {
        let mut mix = LaneMix::default();
        assert_eq!(mix.audible_mask(), 0x3f);
        mix.apply(LaneMixAction::Mute(1));
        assert_eq!(mix.audible_mask(), 0x3d);
        mix.apply(LaneMixAction::Solo(1));
        assert_eq!(mix.audible_mask(), 0);
        mix.apply(LaneMixAction::Solo(5));
        assert_eq!(mix.audible_mask(), 0x20);
        mix.apply(LaneMixAction::Clear);
        assert_eq!(mix.audible_mask(), 0x3f);
    }
}

use eframe::egui;
use leviathan_core::NUM_LANES;
use leviathan_dsp::{LaneChannel, ALL_LANES_AUDIBLE, DEFAULT_LANE_CHANNELS};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(remote = "LaneChannel")]
struct LaneChannelDef {
    level: f32,
    pan: f32,
    delay_send: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StoredLaneChannel(#[serde(with = "LaneChannelDef")] pub LaneChannel);

fn default_channels() -> [StoredLaneChannel; NUM_LANES] {
    DEFAULT_LANE_CHANNELS.map(StoredLaneChannel)
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LaneMix {
    #[serde(default)]
    pub mute_mask: u8,
    #[serde(default)]
    pub solo_mask: u8,
    #[serde(default = "default_channels")]
    pub channels: [StoredLaneChannel; NUM_LANES],
}

impl Default for LaneMix {
    fn default() -> Self {
        Self {
            mute_mask: 0,
            solo_mask: 0,
            channels: default_channels(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum LaneMixAction {
    Mute(usize),
    Solo(usize),
    Clear,
}

impl LaneMix {
    pub fn dsp_channels(self) -> [LaneChannel; NUM_LANES] {
        self.channels.map(|channel| channel.0)
    }

    pub fn audible_mask(self) -> u8 {
        let solo = self.solo_mask & ALL_LANES_AUDIBLE;
        let selected = if solo == 0 { ALL_LANES_AUDIBLE } else { solo };
        selected & !self.mute_mask
    }

    pub fn is_valid(self) -> bool {
        (self.mute_mask | self.solo_mask) & !ALL_LANES_AUDIBLE == 0
            && self.channels.iter().all(|channel| channel.0.is_valid())
    }

    pub fn apply(&mut self, action: LaneMixAction) {
        match action {
            LaneMixAction::Mute(lane) => self.mute_mask ^= 1 << lane,
            LaneMixAction::Solo(lane) => self.solo_mask ^= 1 << lane,
            LaneMixAction::Clear => {
                self.mute_mask = 0;
                self.solo_mask = 0;
            }
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
        mix.channels[0].0.pan = -1.0;
        mix.apply(LaneMixAction::Clear);
        assert_eq!(mix.audible_mask(), 0x3f);
        assert_eq!(mix.channels[0].0.pan, -1.0);
    }
}

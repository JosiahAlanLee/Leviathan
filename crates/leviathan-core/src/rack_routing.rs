use crate::NUM_LANES;

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleRegion {
    pub trim_start: f32,
    pub trim_end: f32,
    pub loop_enabled: bool,
}

impl Default for SampleRegion {
    fn default() -> Self {
        Self {
            trim_start: 0.0,
            trim_end: 1.0,
            loop_enabled: false,
        }
    }
}

impl SampleRegion {
    pub fn is_valid(self) -> bool {
        (0.0..=1.0).contains(&self.trim_start) && (0.0..=1.0).contains(&self.trim_end)
    }

    /// The half-open trim range always includes at least one frame.
    pub fn frames(self, len: usize) -> core::ops::Range<usize> {
        let len = len.max(1);
        let start = ((self.trim_start.clamp(0.0, 1.0) * len as f32) as usize).min(len - 1);
        let end = ((self.trim_end.clamp(0.0, 1.0) * len as f32) as usize).clamp(start + 1, len);
        start..end
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LaneInput {
    #[default]
    Instrument,
    Capture,
    Lane(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ByteLookup {
    #[default]
    Identity,
    Invert,
    Square,
    Fold,
}

impl ByteLookup {
    pub fn map(self, address: u8) -> u8 {
        match self {
            Self::Identity => address,
            Self::Invert => 255 - address,
            Self::Square => ((u16::from(address).pow(2) + 127) / 255) as u8,
            Self::Fold => 2 * address.min(255 - address),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ByteRoute {
    pub enabled: bool,
    pub reset_word: u8,
    pub lookup: ByteLookup,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct RackRouting {
    pub inputs: [LaneInput; NUM_LANES],
    pub region: SampleRegion,
    pub frame_size: u16,
    pub frame_bytebus: bool,
    pub bytebus: ByteRoute,
}

impl Default for RackRouting {
    fn default() -> Self {
        Self {
            inputs: [LaneInput::Instrument; NUM_LANES],
            region: SampleRegion::default(),
            frame_size: 256,
            frame_bytebus: false,
            bytebus: ByteRoute::default(),
        }
    }
}

impl RackRouting {
    /// Order sources before their destinations. Reject cyclic and out-of-bounds routes.
    pub fn render_order(self) -> Option<[usize; NUM_LANES]> {
        if !self.region.is_valid() || self.frame_size < 2 {
            return None;
        }
        let mut order = [0; NUM_LANES];
        let mut scheduled = [false; NUM_LANES];
        for dest in &mut order {
            let lane = self.inputs.iter().enumerate().position(|(lane, input)| {
                !scheduled[lane]
                    && match *input {
                        LaneInput::Lane(source) => {
                            scheduled.get(usize::from(source)) == Some(&true)
                        }
                        _ => true,
                    }
            })?;
            scheduled[lane] = true;
            *dest = lane;
        }
        Some(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_lane_routes_have_an_order_and_cycles_are_rejected() {
        let mut routing = RackRouting::default();
        routing.inputs[0] = LaneInput::Lane(1);
        routing.inputs[1] = LaneInput::Lane(4);
        let order = routing.render_order().unwrap();
        assert_eq!(&order[..4], &[2, 3, 4, 1]);
        routing.inputs[4] = LaneInput::Lane(0);
        assert_eq!(routing.render_order(), None);
        routing.inputs[4] = LaneInput::Lane(17);
        assert_eq!(routing.render_order(), None);
    }
}

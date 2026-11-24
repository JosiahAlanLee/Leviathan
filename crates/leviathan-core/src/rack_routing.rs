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
        (0.0..=1.0).contains(&self.trim_start)
            && (0.0..=1.0).contains(&self.trim_end)
            && self.trim_start <= self.trim_end
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

    #[test]
    fn sample_region_default_spans_full_range_without_loop() {
        let region = SampleRegion::default();
        assert_eq!(region.trim_start, 0.0);
        assert_eq!(region.trim_end, 1.0);
        assert!(!region.loop_enabled);
        assert!(region.is_valid());
    }

    #[test]
    fn sample_region_is_valid_at_range_boundaries() {
        assert!(SampleRegion { trim_start: 0.0, trim_end: 0.0, loop_enabled: false }.is_valid());
        assert!(SampleRegion { trim_start: 0.0, trim_end: 1.0, loop_enabled: true }.is_valid());
        assert!(SampleRegion { trim_start: 1.0, trim_end: 1.0, loop_enabled: false }.is_valid());
    }

    #[test]
    fn sample_region_is_invalid_outside_unit_range() {
        assert!(!SampleRegion { trim_start: -0.1, trim_end: 1.0, loop_enabled: false }.is_valid());
        assert!(!SampleRegion { trim_start: 0.0, trim_end: 1.1, loop_enabled: false }.is_valid());
    }

    #[test]
    fn sample_region_is_valid_rejects_inverted_range() {
        assert!(!SampleRegion { trim_start: 0.8, trim_end: 0.2, loop_enabled: false }.is_valid());
    }

    #[test]
    fn sample_region_is_valid_at_equal_trims() {
        assert!(SampleRegion { trim_start: 0.5, trim_end: 0.5, loop_enabled: false }.is_valid());
    }

    #[test]
    fn sample_region_frames_default_spans_all_frames() {
        let region = SampleRegion::default();
        assert_eq!(region.frames(100), 0..100);
        assert_eq!(region.frames(1), 0..1);
    }

    #[test]
    fn sample_region_frames_clamp_trims_and_guarantee_one_frame() {
        let region = SampleRegion { trim_start: 0.5, trim_end: 0.5, loop_enabled: false };
        assert_eq!(region.frames(100), 50..51);
        let full = SampleRegion { trim_start: 1.0, trim_end: 1.0, loop_enabled: false };
        assert_eq!(full.frames(100), 99..100);
        let empty = SampleRegion::default();
        assert_eq!(empty.frames(0), 0..1);
    }

    #[test]
    fn sample_region_frames_always_include_at_least_one_frame() {
        for len in 0..=64usize {
            for &(start, end) in &[(0.0_f32, 0.0_f32), (0.0, 1.0), (0.5, 0.5), (1.0, 1.0), (0.25, 0.75)] {
                let region = SampleRegion { trim_start: start, trim_end: end, loop_enabled: false };
                let range = region.frames(len);
                assert!(range.start < range.end);
                assert!(range.end <= len.max(1));
            }
        }
    }

    #[test]
    fn lane_input_default_is_instrument() {
        assert_eq!(LaneInput::default(), LaneInput::Instrument);
        assert_eq!(LaneInput::Lane(3), LaneInput::Lane(3));
        assert_ne!(LaneInput::Lane(3), LaneInput::Lane(4));
        assert_ne!(LaneInput::Capture, LaneInput::Instrument);
    }

    #[test]
    fn byte_lookup_identity_is_a_no_op() {
        for address in [0u8, 1, 128, 255] {
            assert_eq!(ByteLookup::Identity.map(address), address);
        }
    }

    #[test]
    fn byte_lookup_invert_mirrors_around_midpoint() {
        assert_eq!(ByteLookup::Invert.map(0), 255);
        assert_eq!(ByteLookup::Invert.map(255), 0);
        assert_eq!(ByteLookup::Invert.map(128), 127);
        for address in [0u8, 1, 100, 128, 200, 255] {
            assert_eq!(ByteLookup::Invert.map(address), 255 - address);
        }
    }

    #[test]
    fn byte_lookup_square_is_monotonic_and_hits_rails() {
        assert_eq!(ByteLookup::Square.map(0), 0);
        assert_eq!(ByteLookup::Square.map(255), 255);
        let mut previous = 0;
        for address in 0..=255u8 {
            let mapped = ByteLookup::Square.map(address);
            assert!(mapped >= previous);
            previous = mapped;
        }
    }

    #[test]
    fn byte_lookup_fold_is_symmetric_and_peaks_at_midpoint() {
        assert_eq!(ByteLookup::Fold.map(0), 0);
        assert_eq!(ByteLookup::Fold.map(255), 0);
        assert_eq!(ByteLookup::Fold.map(127), 254);
        assert_eq!(ByteLookup::Fold.map(128), 254);
        for address in [0u8, 1, 64, 127, 128, 200, 255] {
            assert_eq!(ByteLookup::Fold.map(address), ByteLookup::Fold.map(255 - address));
        }
    }

    #[test]
    fn byte_route_default_is_disabled_identity() {
        let route = ByteRoute::default();
        assert!(!route.enabled);
        assert_eq!(route.reset_word, 0);
        assert_eq!(route.lookup, ByteLookup::Identity);
    }

    #[test]
    fn rack_routing_default_is_all_instrument_256_frames() {
        let routing = RackRouting::default();
        assert!(routing.inputs.iter().all(|input| *input == LaneInput::Instrument));
        assert_eq!(routing.region, SampleRegion::default());
        assert_eq!(routing.frame_size, 256);
        assert!(!routing.frame_bytebus);
        assert_eq!(routing.bytebus, ByteRoute::default());
    }

    #[test]
    fn render_order_default_is_identity_permutation() {
        let order = RackRouting::default().render_order().unwrap();
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3, 4, 5]);
        assert_eq!(order, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn render_order_rejects_frame_size_below_two() {
        let mut routing = RackRouting::default();
        routing.frame_size = 0;
        assert_eq!(routing.render_order(), None);
        routing.frame_size = 1;
        assert_eq!(routing.render_order(), None);
        routing.frame_size = 2;
        assert!(routing.render_order().is_some());
    }

    #[test]
    fn render_order_rejects_invalid_region() {
        let mut routing = RackRouting::default();
        routing.region.trim_start = -0.1;
        assert_eq!(routing.render_order(), None);
        routing.region.trim_start = 0.0;
        routing.region.trim_end = 1.1;
        assert_eq!(routing.render_order(), None);
    }

    #[test]
    fn render_order_rejects_self_and_two_lane_cycles() {
        let mut routing = RackRouting::default();
        routing.inputs[2] = LaneInput::Lane(2);
        assert_eq!(routing.render_order(), None);
        let mut routing = RackRouting::default();
        routing.inputs[0] = LaneInput::Lane(1);
        routing.inputs[1] = LaneInput::Lane(0);
        assert_eq!(routing.render_order(), None);
    }

    #[test]
    fn render_order_rejects_out_of_bounds_lane_sources() {
        let mut routing = RackRouting::default();
        routing.inputs[3] = LaneInput::Lane(NUM_LANES as u8);
        assert_eq!(routing.render_order(), None);
        let mut routing = RackRouting::default();
        routing.inputs[3] = LaneInput::Lane(255);
        assert_eq!(routing.render_order(), None);
    }

    #[test]
    fn render_order_places_sources_before_destinations() {
        let mut routing = RackRouting::default();
        routing.inputs[0] = LaneInput::Lane(1);
        routing.inputs[1] = LaneInput::Lane(4);
        routing.inputs[4] = LaneInput::Lane(2);
        let order = routing.render_order().unwrap();
        let position = |lane: usize| order.iter().position(|&l| l == lane).unwrap();
        assert!(position(4) < position(1));
        assert!(position(1) < position(0));
        assert!(position(2) < position(4));
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, [0, 1, 2, 3, 4, 5]);
    }
}

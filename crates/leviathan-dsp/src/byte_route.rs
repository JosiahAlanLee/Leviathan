use leviathan_core::{rack_routing::ByteRoute, CvFrame};

#[derive(Default)]
pub(super) struct Register {
    high: bool,
    reset_high: bool,
    held: u8,
    initializing: bool,
}

impl Register {
    pub(super) fn step(&mut self, cv: &CvFrame, input: Option<u8>, route: ByteRoute) -> u8 {
        let running = cv.run_v > 2.5;
        let high = running && cv.bytebus_v[8] > 5.0;
        let reset = cv.reset_v > 2.5;
        if reset && !self.reset_high {
            self.held = route.reset_word;
        } else if high && !self.high && !self.initializing {
            if let Some(word) = input {
                self.held = word;
            }
        }
        self.reset_high = reset;
        self.high = high;
        self.initializing = false;
        route.lookup.map(self.held)
    }

    pub(super) fn reset(&mut self, seed: u8) {
        self.held = seed;
        self.high = false;
        self.reset_high = false;
        self.initializing = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leviathan_core::{encode_bytebus_frame, rack_routing::ByteLookup};

    #[test]
    fn holds_present_zero_and_reacquires_even_if_the_word_repeats() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 41,
            lookup: ByteLookup::Invert,
        };
        register.reset(route.reset_word);
        let mut step = |high, word, run| {
            let cv = CvFrame {
                run_v: if run { 10.0 } else { 0.0 },
                bytebus_v: encode_bytebus_frame(word, high),
                ..CvFrame::default()
            };
            register.step(&cv, Some(word), route)
        };
        assert_eq!(step(false, 99, true), 214);
        assert_eq!(step(true, 0, true), 255);
        assert_eq!(step(true, 99, true), 255);
        assert_eq!(step(false, 99, true), 255);
        assert_eq!(step(true, 0, true), 255);
        assert_eq!(step(false, 17, false), 255);
        assert_eq!(step(false, 17, true), 255);
        register.reset(17);
        assert_eq!(register.step(&CvFrame::default(), None, route), 238);
    }

    #[test]
    fn reset_seed_wins_over_an_edge_and_is_released_on_the_next_edge() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 44,
            lookup: ByteLookup::Identity,
        };
        let mut cv = CvFrame {
            run_v: 10.0,
            reset_v: 10.0,
            bytebus_v: encode_bytebus_frame(0, true),
            ..CvFrame::default()
        };
        assert_eq!(register.step(&cv, Some(0), route), 44);
        cv.bytebus_v[8] = 0.0;
        assert_eq!(register.step(&cv, Some(0), route), 44);
        cv.reset_v = 0.0;
        cv.bytebus_v[8] = 10.0;
        assert_eq!(register.step(&cv, Some(0), route), 0);
    }

    #[test]
    fn reset_seed_is_released_on_the_next_reset_edge_only_once() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 7,
            lookup: ByteLookup::Identity,
        };
        register.reset(7);
        let cv = CvFrame {
            reset_v: 10.0,
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(0, true),
            ..CvFrame::default()
        };
        assert_eq!(register.step(&cv, Some(200), route), 7);
        // Reset held high: no new edge, no new held word even though input repeats.
        assert_eq!(register.step(&cv, Some(200), route), 7);
        // Rising edge again after release re-seeds.
        let mut released = cv;
        released.reset_v = 0.0;
        released.bytebus_v[8] = 0.0;
        assert_eq!(register.step(&released, Some(200), route), 7);
        let mut edged = released;
        edged.reset_v = 10.0;
        assert_eq!(register.step(&edged, Some(200), route), 7);
    }

    #[test]
    fn reset_applies_even_when_run_is_low() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 9,
            lookup: ByteLookup::Identity,
        };
        let cv = CvFrame {
            reset_v: 10.0,
            run_v: 0.0,
            bytebus_v: encode_bytebus_frame(0, true),
            ..CvFrame::default()
        };
        assert_eq!(register.step(&cv, Some(3), route), 9);
    }

    #[test]
    fn seed_word_is_not_overwritten_by_a_high_edge_right_after_reset() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 13,
            lookup: ByteLookup::Identity,
        };
        register.reset(13);
        let cv = CvFrame {
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(250, true),
            ..CvFrame::default()
        };
        // Initializing edge: the seed wins, not the first input word.
        assert_eq!(register.step(&cv, Some(250), route), 13);
        // Once initialized, the next rising edge acquires the input word.
        let cv2 = CvFrame {
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(0, false),
            ..CvFrame::default()
        };
        assert_eq!(register.step(&cv2, Some(250), route), 13);
        let cv3 = CvFrame {
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(250, true),
            ..CvFrame::default()
        };
        assert_eq!(register.step(&cv3, Some(250), route), 250);
    }

    #[test]
    fn held_word_is_stable_across_low_steps_and_ignores_missing_input() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 11,
            lookup: ByteLookup::Identity,
        };
        register.reset(11);
        let mut cv = CvFrame {
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(77, true),
            ..CvFrame::default()
        };
        // Initializing edge: no acquire, held stays the seed.
        assert_eq!(register.step(&cv, Some(77), route), 11);
        cv.bytebus_v[8] = 0.0;
        assert_eq!(register.step(&cv, None, route), 11);
        cv.bytebus_v[8] = 10.0;
        // Rising edge with None input: held is not acquired, seed stays.
        assert_eq!(register.step(&cv, None, route), 11);
        // Steady high: no new edge, held unchanged.
        assert_eq!(register.step(&cv, None, route), 11);
        cv.bytebus_v[8] = 0.0;
        assert_eq!(register.step(&cv, Some(9), route), 11);
        cv.bytebus_v[8] = 10.0;
        // Rising edge with an input word acquires it.
        assert_eq!(register.step(&cv, Some(9), route), 9);
    }

    #[test]
    fn min_and_max_reset_words_are_mapped_through_the_lookup() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 0,
            lookup: ByteLookup::Square,
        };
        register.reset(0);
        assert_eq!(register.step(&CvFrame::default(), None, route), 0);
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 255,
            lookup: ByteLookup::Square,
        };
        register.reset(255);
        assert_eq!(register.step(&CvFrame::default(), None, route), 255);
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 0,
            lookup: ByteLookup::Fold,
        };
        register.reset(0);
        assert_eq!(register.step(&CvFrame::default(), None, route), 0);
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 255,
            lookup: ByteLookup::Fold,
        };
        register.reset(255);
        assert_eq!(register.step(&CvFrame::default(), None, route), 0);
    }

    #[test]
    fn held_word_is_mapped_through_every_lookup() {
        let cv = CvFrame {
            run_v: 10.0,
            bytebus_v: encode_bytebus_frame(100, true),
            ..CvFrame::default()
        };
        for (lookup, expected) in [
            (ByteLookup::Identity, 100),
            (ByteLookup::Invert, 155),
            (ByteLookup::Square, 39),
            (ByteLookup::Fold, 200),
        ] {
            let route = ByteRoute {
                enabled: true,
                reset_word: 0,
                lookup,
            };
            let mut register = Register::default();
            // First step is the rising edge: it acquires the input word.
            assert_eq!(register.step(&cv, Some(100), route), expected);
        }
    }

    #[test]
    fn lookup_of_held_word_is_deterministic_across_repeated_steps() {
        let mut register = Register::default();
        let route = ByteRoute {
            enabled: true,
            reset_word: 17,
            lookup: ByteLookup::Square,
        };
        register.reset(17);
        let cv = CvFrame::default();
        let first = register.step(&cv, None, route);
        let second = register.step(&cv, None, route);
        assert_eq!(first, second);
        assert_eq!(first, route.lookup.map(17));
    }
}

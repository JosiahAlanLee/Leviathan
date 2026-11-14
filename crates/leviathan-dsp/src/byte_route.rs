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
}

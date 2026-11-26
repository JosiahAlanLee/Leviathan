//! Tablet controls over the shared tracker state. No mobile-only audio path.

use leviathan_core::{NoteCell, MAX_NUDGE};
use leviathan_dsp::{LanePatch, MachineKind};

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrubMode {
    #[default]
    Note,
    Velocity,
    Nudge,
    Lock(usize),
}

fn lock_param(cell: NoteCell, lane: &LanePatch, slot: usize) -> Option<(&'static str, f32)> {
    let machine = MachineKind::from_patch_and_lock(lane, &cell.lock);
    // This slot packs two controls into a value outside the normal macro range.
    if machine == MachineKind::OtisTwin && slot == crate::param_ui::MODE_GRIT_SLOT {
        return None;
    }
    let param = machine.spec().params.get(slot)?;
    let value = if cell.lock.has_param(slot) {
        cell.lock.values[slot]
    } else {
        lane.step_base_params(machine)[slot]
    };
    Some((param.name, value))
}

#[derive(Clone, Copy)]
pub struct ScrubDrag {
    pub mode: ScrubMode,
    initial: NoteCell,
    pub recorded_undo: bool,
}

impl ScrubDrag {
    pub fn new(mode: ScrubMode, initial: NoteCell) -> Self {
        Self {
            mode,
            initial,
            recorded_undo: false,
        }
    }

    /// Interpret total movement from the start, so slow drags don't lose
    /// sub-step motion and returning to the origin restores the original cell.
    pub fn cell_at(self, up: f32, lane: &LanePatch) -> NoteCell {
        let start = self.initial;
        let mut cell = start;
        if !up.is_finite() {
            return start;
        }
        match self.mode {
            ScrubMode::Note => {
                let steps = (up / 8.0).round() as i32;
                if steps != 0 {
                    cell.midi =
                        (i32::from(start.midi) + steps.clamp(-127, 127)).clamp(0, 127) as u8;
                }
            }
            ScrubMode::Velocity => {
                let steps = (up / 3.0).round() as i32;
                cell.velocity =
                    (i32::from(start.velocity) + steps.clamp(-127, 127)).clamp(0, 127) as u8;
            }
            ScrubMode::Nudge => {
                let steps = (up / 5.0).round() as i32;
                let max = i32::from(MAX_NUDGE);
                cell.micro_offset = (i32::from(start.micro_offset) + steps.clamp(-2 * max, 2 * max))
                    .clamp(-max, max) as i8;
            }
            ScrubMode::Lock(slot) => {
                if let Some((_, base)) = lock_param(start, lane, slot) {
                    let delta = (up / 2.0).round() * 0.01;
                    if delta != 0.0 && base.is_finite() {
                        let value = (base + delta).clamp(0.0, 1.0);
                        if value != base {
                            cell.lock.mask |= 1 << slot;
                            cell.lock.values[slot] = value;
                        }
                    }
                }
            }
        }
        cell
    }
}

#[derive(Default)]
pub struct TouchFill {
    latched: bool,
    held: bool,
    contact: Option<(egui::TouchDeviceId, egui::TouchId)>,
}

impl TouchFill {
    pub fn update(&mut self, toggle_latch: bool, held: bool) -> bool {
        self.latched ^= toggle_latch;
        self.held = held;
        self.latched || self.held
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, enabled: bool) -> bool {
        let (toggle, mouse_held, hit) = ui
            .add_enabled_ui(enabled, |ui| {
                let latch = ui
                    .add_sized(
                        [92.0, 40.0],
                        egui::Button::new("FILL latch").selected(self.latched),
                    )
                    .on_hover_text("Stay in FILL until tapped again.");
                let hold = ui
                    .add_sized(
                        [92.0, 40.0],
                        egui::Button::new("FILL hold").selected(self.held),
                    )
                    .on_hover_text("Temporary FILL while pressed.");
                (
                    latch.clicked(),
                    hold.is_pointer_button_down_on(),
                    hold.rect.intersect(ui.clip_rect()),
                )
            })
            .inner;

        let held = ui.input(|input| {
            if !enabled || !input.focused {
                self.contact = None;
                return false;
            }
            // Track the hold contact independently of egui's primary pointer so
            // FILL can be held even if the other hand is already scrubbing.
            let mut saw_touch = false;
            for event in &input.events {
                if let egui::Event::Touch {
                    device_id,
                    id,
                    phase,
                    pos,
                    ..
                } = event
                {
                    let key = (*device_id, *id);
                    saw_touch = true;
                    match phase {
                        egui::TouchPhase::Start if self.contact.is_none() && hit.contains(*pos) => {
                            self.contact = Some(key);
                        }
                        egui::TouchPhase::End | egui::TouchPhase::Cancel
                            if self.contact == Some(key) =>
                        {
                            self.contact = None
                        }
                        _ => {}
                    }
                }
            }
            if !input.any_touches() {
                self.contact = None;
            }
            self.contact.is_some()
                || (!saw_touch
                    && !input.any_touches()
                    && input.pointer.latest_pos().is_some()
                    && mouse_held)
        });
        self.update(enabled && toggle, held)
    }
}

#[derive(Default)]
pub struct TabletScrub {
    mode: ScrubMode,
    lock_slot: usize,
    gesture: Option<((usize, usize, usize), f32, ScrubDrag)>,
}

impl TabletScrub {
    pub fn end_drag(&mut self) {
        self.gesture = None;
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        address: (usize, usize, usize),
        initial: NoteCell,
        lane: &LanePatch,
        enabled: bool,
    ) -> Option<(ScrubDrag, f32)> {
        let (pattern, lane_index, row) = address;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "Edit {pattern:02X} · L{} · Row {row:02X}",
                lane_index + 1
            ));
            for (mode, label) in [
                (ScrubMode::Note, "Note"),
                (ScrubMode::Velocity, "Velocity"),
                (ScrubMode::Nudge, "Nudge"),
                (ScrubMode::Lock(self.lock_slot), "Lock"),
            ] {
                if ui
                    .add_sized(
                        [54.0, 40.0],
                        egui::Button::new(label).selected(self.mode == mode),
                    )
                    .clicked()
                {
                    self.mode = mode;
                }
            }
            if matches!(self.mode, ScrubMode::Lock(_)) {
                egui::ComboBox::from_id_salt("tablet_scrub_lock_slot")
                    .selected_text(format!("P{}", self.lock_slot))
                    .show_ui(ui, |ui| {
                        for slot in 0..initial.lock.values.len() {
                            if let Some((name, _)) = lock_param(initial, lane, slot) {
                                ui.selectable_value(
                                    &mut self.lock_slot,
                                    slot,
                                    format!("P{slot} {name}"),
                                );
                            }
                        }
                    });
                self.mode = ScrubMode::Lock(self.lock_slot);
            }
        });

        // Scrub an active note in this strip instead of stealing the grid's
        // drag-to-scroll gesture. The existing keyboard strip inserts notes.
        let allowed = enabled
            && initial.active
            && (self.mode != ScrubMode::Note || !initial.air)
            && !matches!(self.mode, ScrubMode::Lock(slot) if lock_param(initial, lane, slot).is_none());
        let text = if allowed {
            format!("{}  ↕", display_value(self.mode, initial, lane))
        } else if !enabled || !initial.active {
            "Select a populated step to scrub".to_owned()
        } else if self.mode == ScrubMode::Note && initial.air {
            "AIR has no pitch to scrub".to_owned()
        } else {
            "Edit this macro in Voice / Trig".to_owned()
        };
        let response = ui.add_enabled(
            allowed,
            egui::Button::new(text)
                .min_size(egui::vec2(200.0, 46.0))
                .sense(egui::Sense::drag()),
        );

        if self
            .gesture
            .as_ref()
            .is_some_and(|(a, _, drag)| *a != address || drag.mode != self.mode)
            || !allowed
            || !pointer_available(ui)
        {
            self.end_drag();
            return None;
        }
        if response.drag_started() {
            if let Some(pos) = ui.input(|input| input.pointer.press_origin()) {
                self.gesture = Some((address, pos.y, ScrubDrag::new(self.mode, initial)));
            }
        }
        if response.dragged() {
            if let (Some((_, origin_y, drag)), Some(pos)) =
                (self.gesture, response.interact_pointer_pos())
            {
                let up = origin_y - pos.y;
                show_loupe(
                    ui.ctx(),
                    pos,
                    display_value(drag.mode, drag.cell_at(up, lane), lane),
                );
                return Some((drag, up));
            }
        } else {
            self.end_drag();
        }
        None
    }

    pub fn mark_edit_recorded(&mut self) {
        if let Some((_, _, drag)) = &mut self.gesture {
            drag.recorded_undo = true;
        }
    }
}

fn display_value(mode: ScrubMode, cell: NoteCell, lane: &LanePatch) -> String {
    match mode {
        ScrubMode::Note => format!("Note {}", crate::midi_label(cell.midi)),
        ScrubMode::Velocity => format!("Velocity {}", cell.velocity),
        ScrubMode::Nudge => format!("Nudge {:+}/48", cell.micro_offset),
        ScrubMode::Lock(slot) => match lock_param(cell, lane, slot) {
            Some((name, val)) => {
                let source = if cell.lock.has_param(slot) {
                    "Lock"
                } else {
                    "Lane"
                };
                format!("{name} {val:.2} · {source}")
            }
            None => "Edit this macro in Voice / Trig".to_owned(),
        },
    }
}

// On Android, egui-winit reports a canceled primary touch as PointerGone,
// without a matching button-up. Do not leave a momentary mode latched.
fn pointer_available(ui: &egui::Ui) -> bool {
    ui.input(|input| input.focused && input.pointer.latest_pos().is_some())
}

fn show_loupe(ctx: &egui::Context, pointer: egui::Pos2, value: String) {
    let screen = ctx.screen_rect();
    let above = pointer.y - screen.top() >= 100.0;
    let offset = egui::vec2(0.0, if above { -64.0 } else { 64.0 });
    egui::Area::new(egui::Id::new("tablet_scrub_loupe"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .movable(false)
        .pivot(if above {
            egui::Align2::CENTER_BOTTOM
        } else {
            egui::Align2::CENTER_TOP
        })
        .fixed_pos(pointer + offset)
        .constrain_to(screen)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(egui::RichText::new(value).strong().size(21.0));
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_drag_is_bounded_and_reversible() {
        let initial = NoteCell {
            active: true,
            midi: 57,
            velocity: 63,
            ..Default::default()
        };
        let lane = LanePatch::for_machine_with_calibration(MachineKind::KickDrum);
        let note = ScrubDrag::new(ScrubMode::Note, initial);
        assert_eq!(note.cell_at(16.0, &lane).midi, 59);
        assert_eq!(note.cell_at(0.0, &lane), initial);
        assert_eq!(note.cell_at(f32::MAX, &lane).midi, 127);
        assert_eq!(note.cell_at(f32::NAN, &lane), initial);
        assert_eq!(
            ScrubDrag::new(ScrubMode::Velocity, initial)
                .cell_at(-30.0, &lane)
                .velocity,
            53
        );
        assert_eq!(
            ScrubDrag::new(ScrubMode::Nudge, initial)
                .cell_at(500.0, &lane)
                .micro_offset,
            MAX_NUDGE
        );
    }

    #[test]
    fn macro_drag_starts_from_inherited_value_and_preserves_packed_slot() {
        let initial = NoteCell {
            active: true,
            ..Default::default()
        };
        let lane = LanePatch::for_machine_with_calibration(MachineKind::KickDrum);
        let delta = ScrubDrag::new(ScrubMode::Lock(0), initial);
        let edited = delta.cell_at(20.0, &lane);
        assert!(edited.lock.has_param(0));
        assert!((edited.lock.values[0] - (lane.params[0] + 0.1)).abs() < 0.00001);
        assert_eq!(delta.cell_at(0.0, &lane), initial);
        let twin = LanePatch::for_machine_with_calibration(MachineKind::OtisTwin);
        assert_eq!(
            ScrubDrag::new(ScrubMode::Lock(crate::param_ui::MODE_GRIT_SLOT), initial)
                .cell_at(50.0, &twin),
            initial
        );
    }

    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut draw: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, &mut draw);
            },
        )
    }

    fn text_pos(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing control {label:?}"))
    }

    fn press(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn hold_button_releases_on_canceled_touch_without_button_up() {
        let ctx = egui::Context::default();
        let mut fill = TouchFill::default();
        let mut effective = false;
        let mut draw = |ui: &mut egui::Ui| {
            effective = fill.draw(ui, true);
        };
        let warm = frame(&ctx, vec![], &mut draw);
        let pos = text_pos(&warm, "FILL hold");
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(pos), press(pos)],
            &mut draw,
        );
        assert!(effective);
        frame(&ctx, vec![egui::Event::PointerGone], |ui| {
            effective = fill.draw(ui, true);
        });
        assert!(!effective);
    }

    #[test]
    fn a_second_touch_can_hold_fill_and_cancel_clears_it() {
        let ctx = egui::Context::default();
        let mut fill = TouchFill::default();
        let mut effective = false;
        let warm = frame(&ctx, vec![], |ui| {
            effective = fill.draw(ui, true);
        });
        let hold = text_pos(&warm, "FILL hold");
        let outside = egui::pos2(550.0, 300.0);
        let touch = |id, phase, pos| egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id),
            phase,
            pos,
            force: None,
        };
        frame(
            &ctx,
            vec![
                egui::Event::PointerMoved(outside),
                press(outside),
                touch(0, egui::TouchPhase::Start, outside),
                touch(1, egui::TouchPhase::Start, hold),
            ],
            |ui| {
                effective = fill.draw(ui, true);
            },
        );
        assert!(effective);
        frame(&ctx, vec![touch(1, egui::TouchPhase::Cancel, hold)], |ui| {
            effective = fill.draw(ui, true);
        });
        assert!(!effective);
    }

    #[test]
    fn scrub_widget_uses_press_origin_and_shows_loupe() {
        let ctx = egui::Context::default();
        let mut scrub = TabletScrub::default();
        let initial = NoteCell {
            active: true,
            midi: 60,
            ..Default::default()
        };
        let lane = LanePatch::for_machine_with_calibration(MachineKind::KickDrum);
        let warm = frame(&ctx, vec![], |ui| {
            scrub.draw(ui, (0, 0, 0), initial, &lane, true);
        });
        let pos = text_pos(&warm, "Note C-4  ↕");
        frame(
            &ctx,
            vec![egui::Event::PointerMoved(pos), press(pos)],
            |ui| {
                scrub.draw(ui, (0, 0, 0), initial, &lane, true);
            },
        );
        let moved = pos - egui::vec2(0.0, 16.0);
        let mut changed = None;
        frame(&ctx, vec![egui::Event::PointerMoved(moved)], |ui| {
            changed = scrub.draw(ui, (0, 0, 0), initial, &lane, true);
        });
        let (drag, delta) = changed.expect("scrub drag");
        assert_eq!(drag.cell_at(delta, &lane).midi, 62);
        scrub.mark_edit_recorded();
        let (_, _, recorded) = scrub.gesture.expect("live scrub");
        assert!(recorded.recorded_undo);
        let out = frame(&ctx, vec![], |ui| {
            scrub.draw(ui, (0, 0, 0), initial, &lane, true);
        });
        text_pos(&out, "Note D-4");
        frame(&ctx, vec![egui::Event::PointerGone], |ui| {
            assert!(scrub.draw(ui, (0, 0, 0), initial, &lane, true).is_none());
        });
        assert!(scrub.gesture.is_none());
    }

    #[test]
    fn releasing_hold_preserves_latch_and_release_without_latch_clears_fill() {
        let mut fill = TouchFill::default();
        assert!(fill.update(false, true));
        assert!(!fill.update(false, false));
        assert!(fill.update(true, false));
        assert!(fill.update(false, true));
        assert!(fill.update(false, false));
        assert!(!fill.update(true, false));
    }
}

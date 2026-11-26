use std::{
    ops::Range,
    path::PathBuf,
    sync::{mpsc, Arc},
};

use eframe::egui;
use leviathan_dsp::otis_kit::{MAX_FRAMES, SLOTS};

use crate::{
    audition,
    kit::{Kit, KitSample},
    wav,
};

type Peak = (i16, i16);

struct Source {
    sample: Arc<KitSample>,
    peaks: Vec<Vec<Peak>>,
}

impl Source {
    fn new(sample: Arc<KitSample>) -> Self {
        let mut peaks = vec![sample
            .pcm
            .chunks(16)
            .map(|s| (*s.iter().min().unwrap(), *s.iter().max().unwrap()))
            .collect::<Vec<_>>()];
        while peaks.last().is_some_and(|level| level.len() > 1) {
            let next = peaks
                .last()
                .unwrap()
                .chunks(2)
                .map(|p| {
                    p.iter()
                        .fold((i16::MAX, i16::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)))
                })
                .collect();
            peaks.push(next);
        }
        Self { sample, peaks }
    }

    fn peak(&self, range: Range<usize>) -> Peak {
        let mut pos = range.start;
        let mut out = (i16::MAX, i16::MIN);
        while pos < range.end {
            let remaining = range.end - pos;
            let mut peak = (self.sample.pcm[pos], self.sample.pcm[pos]);
            let mut stride = 1;
            for level in (0..self.peaks.len()).rev() {
                let size = 16 << level;
                if size <= remaining && pos % size == 0 {
                    peak = self.peaks[level][pos / size];
                    stride = size;
                    break;
                }
            }
            out = (out.0.min(peak.0), out.1.max(peak.1));
            pos += stride;
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Start,
    End,
    Split(usize),
}

#[derive(Default)]
pub struct VisualSampler {
    pending: Option<mpsc::Receiver<Result<Source, String>>>,
    source: Option<Source>,
    pub status: String,
    #[cfg(not(target_os = "android"))]
    path: String,
    start: usize,
    end: usize,
    splits: Vec<usize>,
    marker: Option<Marker>,
    head: usize,
    window: Range<usize>,
    drag: Option<(Option<Marker>, usize, f32)>,
    pub lane_slot: usize,
}

#[derive(Default)]
pub struct Actions {
    #[cfg(target_os = "android")]
    pub open: bool,
    pub audio: Option<Option<audition::Request>>,
    pub commit: Option<usize>,
    pub undo: bool,
    pub enter: bool,
    pub hear_slot: bool,
}

impl VisualSampler {
    pub fn load(
        &mut self,
        path: PathBuf,
        name: Option<String>,
        temporary: bool,
        ctx: &egui::Context,
    ) {
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.status = "Opening WAV…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = wav::load_kit_wav(&path).and_then(|(fallback, rate, pcm)| {
                KitSample::new(name.unwrap_or(fallback), rate, pcm).map(Source::new)
            });
            if temporary {
                let _ = std::fs::remove_file(path);
            }
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub fn accepts_drop(&self, ctx: &egui::Context) -> bool {
        let path = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.as_ref())
                .find(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
                })
                .cloned()
        });
        path.is_some()
    }

    pub fn use_drop(&mut self, ctx: &egui::Context) {
        let path = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.as_ref())
                .find(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
                })
                .cloned()
        });
        if let Some(path) = path {
            self.load(path, None, false, ctx);
        }
    }

    pub fn poll(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let result = match pending.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err("WAV import did not finish.".into()),
        };
        self.pending = None;
        match result {
            Ok(source) => {
                let n = source.sample.pcm.len();
                self.start = 0;
                self.end = n;
                self.splits.clear();
                self.window = 0..n;
                self.head = 0;
                self.marker = Some(Marker::Start);
                self.drag = None;
                self.status.clear();
                self.source = Some(source);
                true
            }
            Err(error) => {
                self.status = error;
                false
            }
        }
    }

    fn ranges(&self) -> impl Iterator<Item = Range<usize>> + '_ {
        std::iter::once(self.start)
            .chain(self.splits.iter().copied())
            .zip(self.splits.iter().copied().chain(std::iter::once(self.end)))
            .map(|(a, b)| a..b)
    }

    fn move_marker(&mut self, m: Marker, frame: usize, length: usize) {
        match m {
            Marker::Start => {
                self.start = frame.min(self.splits.first().copied().unwrap_or(self.end) - 2)
            }
            Marker::End => {
                self.end = frame.clamp(
                    self.splits.last().copied().unwrap_or(self.start) + 2,
                    length,
                )
            }
            Marker::Split(i) => {
                let lo = if i == 0 {
                    self.start
                } else {
                    self.splits[i - 1]
                };
                let hi = self.splits.get(i + 1).copied().unwrap_or(self.end);
                self.splits[i] = frame.clamp(lo + 2, hi - 2);
            }
        }
    }

    fn add_split(&mut self) -> Result<(), &'static str> {
        if self.splits.len() >= SLOTS - 1 {
            return Err("The kit supports up to eight slices.");
        }
        let i = self.splits.partition_point(|&frame| frame < self.head);
        let lo = if i == 0 {
            self.start
        } else {
            self.splits[i - 1]
        };
        let hi = self.splits.get(i).copied().unwrap_or(self.end);
        if self.head < lo + 2 || self.head > hi - 2 {
            return Err("Place the cursor at least two frames from a boundary.");
        }
        self.splits.insert(i, self.head);
        self.marker = Some(Marker::Split(i));
        Ok(())
    }

    fn marker_position(&self, marker: Marker) -> usize {
        match marker {
            Marker::Start => self.start,
            Marker::End => self.end,
            Marker::Split(i) => self.splits[i],
        }
    }

    pub fn make_kit(&self, kit: &Kit, first: usize) -> Result<Kit, String> {
        let source = self.source.as_ref().ok_or("Open a WAV first.")?;
        let count = self.splits.len() + 1;
        let end = first
            .checked_add(count)
            .filter(|&end| end <= SLOTS)
            .ok_or("Not enough consecutive kit slots.")?;
        if kit.samples[first..end].iter().any(Option::is_some) {
            return Err(
                "Choose consecutive empty slots; existing hits are never overwritten.".into(),
            );
        }
        let used = kit
            .samples
            .iter()
            .flatten()
            .map(|s| s.pcm.len())
            .sum::<usize>();
        if used + (self.end - self.start) > MAX_FRAMES {
            return Err("These slices would exceed the kit frame budget.".into());
        }
        let mut next = kit.clone();
        for (n, range) in self.ranges().enumerate() {
            let suffix = format!(" · {}", n + 1);
            let mut name = source.sample.name.clone();
            while name.len() + suffix.len() > 256 {
                name.pop();
            }
            name.push_str(&suffix);
            next.samples[first + n] = Some(KitSample::new(
                name,
                source.sample.rate,
                source.sample.pcm[range].to_vec(),
            )?);
            next.params[first + n] = Default::default();
        }
        if !next.is_valid() {
            return Err("The proposed kit failed validation.".into());
        }
        Ok(next)
    }

    fn set_window(&mut self, focus: usize, len: usize, length: usize) {
        let len = len.clamp(2, length);
        let start = focus.saturating_sub(len / 2).min(length - len);
        self.window = start..start + len;
    }

    pub fn draw(&mut self, ui: &mut egui::Ui, kit: &Kit, live: bool, undo_ready: bool) -> Actions {
        let mut actions = Actions::default();
        ui.heading("Waveform editor");
        #[cfg(not(target_os = "android"))]
        ui.horizontal_wrapped(|ui| {
            ui.label("WAV path or drop a WAV here");
            let response = ui.text_edit_singleline(&mut self.path);
            if ui.button("Open WAV").clicked()
                || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                if !self.path.trim().is_empty() {
                    self.load(self.path.trim().into(), None, false, ui.ctx());
                }
            }
        });
        #[cfg(target_os = "android")]
        {
            actions.open = ui.button("Open WAV from Files").clicked();
        }
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        let Some(source) = &self.source else {
            return actions;
        };
        let source_name = source.sample.name.clone();
        let rate = source.sample.rate;
        let length = source.sample.pcm.len();
        ui.label(format!(
            "{source_name} · {rate} Hz · {:.3} s",
            length as f64 / f64::from(rate)
        ));
        ui.horizontal_wrapped(|ui| {
            for (label, marker) in [("Start", Marker::Start), ("End", Marker::End)] {
                if ui
                    .selectable_label(self.marker == Some(marker), label)
                    .clicked()
                {
                    self.marker = Some(marker);
                }
            }
            if ui.button("Zoom +").clicked() {
                let center = self
                    .marker
                    .map(|m| self.marker_position(m))
                    .unwrap_or(self.head);
                self.set_window(center, self.window.len() / 2, length);
            }
            if ui.button("Zoom −").clicked() {
                let center = self
                    .marker
                    .map(|m| self.marker_position(m))
                    .unwrap_or(self.head);
                self.set_window(center, self.window.len().saturating_mul(2), length);
            }
            if ui.button("Fit").clicked() {
                self.window = 0..length;
            }
            for (label, delta) in [("−1 frame", -1isize), ("+1 frame", 1)] {
                if ui.button(label).clicked() {
                    if let Some(marker) = self.marker {
                        let frame = self.marker_position(marker).saturating_add_signed(delta);
                        self.move_marker(marker, frame, length);
                    } else {
                        self.head = self.head.saturating_add_signed(delta).min(length);
                    }
                }
            }
        });
        self.draw_wave(ui, length);
        ui.monospace(format!(
            "Trim {}..{} · Cursor {} · {:.3} s",
            self.start,
            self.end,
            self.head,
            self.head as f64 / f64::from(rate)
        ));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add split at cursor").clicked() {
                self.status = self.add_split().err().unwrap_or("").into();
            }
            if let Some(Marker::Split(i)) = self.marker {
                if ui.button("Remove selected split").clicked() {
                    self.splits.remove(i);
                    self.marker = None;
                }
            }
            for i in 0..self.splits.len() {
                let marker = Marker::Split(i);
                if ui
                    .selectable_label(self.marker == Some(marker), format!("Split {}", i + 1))
                    .clicked()
                {
                    self.marker = Some(marker);
                }
            }
        });
        let source = &self.source.as_ref().unwrap().sample;
        ui.horizontal_wrapped(|ui| {
            let play =
                |ui: &mut egui::Ui, label: String, range: Range<usize>, actions: &mut Actions| {
                    if ui.add_enabled(live, egui::Button::new(label)).clicked() {
                        actions.audio = Some(Some(audition::Request {
                            sample: audition::Sample::Kit(source.clone()),
                            range,
                            reverse: false,
                        }));
                    }
                };
            play(ui, "Play trim".into(), self.start..self.end, &mut actions);
            for (n, range) in self.ranges().enumerate() {
                play(ui, format!("Play {}", n + 1), range, &mut actions);
            }
            if ui
                .add_enabled(live, egui::Button::new("Stop audition"))
                .clicked()
            {
                actions.audio = Some(None);
            }
        });
        let count = self.splits.len() + 1;
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("slice_slot")
                .selected_text(format!("Slot {}", self.lane_slot + 1))
                .show_ui(ui, |ui| {
                    for i in 0..SLOTS {
                        let label = kit.samples[i]
                            .as_ref()
                            .map(|s| s.name.as_str())
                            .unwrap_or("empty");
                        ui.selectable_value(&mut self.lane_slot, i, format!("{} · {label}", i + 1));
                    }
                });
            if self.lane_slot + count <= SLOTS {
                ui.label(format!(
                    "Destination {}–{}",
                    self.lane_slot + 1,
                    self.lane_slot + count
                ));
            } else {
                ui.label("Not enough consecutive slots");
            }
            if ui.button("Split to Kit").clicked() {
                actions.commit = Some(self.lane_slot);
            }
            actions.undo = ui
                .add_enabled(undo_ready, egui::Button::new("Undo split"))
                .clicked();
            let filled = kit.samples[self.lane_slot].is_some();
            actions.enter = ui
                .add_enabled(filled, egui::Button::new("Enter hit"))
                .clicked();
            actions.hear_slot = ui
                .add_enabled(filled && live, egui::Button::new("Hear kit slot"))
                .clicked();
        });
        actions
    }

    fn draw_wave(&mut self, ui: &mut egui::Ui, length: usize) {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width().max(1.0), 155.0),
            egui::Sense::click_and_drag(),
        );
        let window = self.window.clone();
        let x_at = |frame: usize| {
            rect.left()
                + rect.width() * ((frame as f64 - window.start as f64) / window.len() as f64) as f32
        };
        let frame_at = |x: f32| {
            window.start
                + ((x - rect.left()) / rect.width())
                    .clamp(0.0, 1.0)
                    .mul_add(window.len() as f32, 0.0)
                    .round() as usize
        };
        let markers = std::iter::once(Marker::Start)
            .chain((0..self.splits.len()).map(Marker::Split))
            .chain(std::iter::once(Marker::End));
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 4.0, egui::Color32::from_gray(23));
        let x0 = x_at(self.start).clamp(rect.left(), rect.right());
        let x1 = x_at(self.end).clamp(rect.left(), rect.right());
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            egui::Color32::from_rgb(25, 48, 54),
        );
        let source = self.source.as_ref().unwrap();
        let columns = (rect.width().round() as usize)
            .clamp(1, 1600)
            .min(window.len());
        let y =
            |sample: i16| rect.center().y - f32::from(sample) / 32768.0 * (rect.height() * 0.46);
        let stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(105, 202, 192));
        let mut prior = None;
        for i in 0..columns {
            let a = window.start + i * window.len() / columns;
            let b = window.start + (i + 1) * window.len() / columns;
            let (lo, hi) = source.peak(a..b);
            let x = x_at(a);
            if columns == window.len() {
                let point = egui::pos2(x, y(lo));
                if let Some(prev) = prior {
                    painter.line_segment([prev, point], stroke);
                }
                painter.circle_filled(point, 1.75, stroke.color);
                prior = Some(point);
            } else {
                painter.line_segment([egui::pos2(x, y(lo)), egui::pos2(x, y(hi))], stroke);
            }
        }
        let draw_line = |x: f32, color: egui::Color32, width: f32| {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(width, color),
            );
        };
        draw_line(x_at(self.head), egui::Color32::WHITE, 1.0);
        for marker in markers.clone() {
            draw_line(
                x_at(self.marker_position(marker)),
                if Some(marker) == self.marker {
                    egui::Color32::GOLD
                } else {
                    egui::Color32::LIGHT_BLUE
                },
                2.0,
            );
        }
        if let Some(pos) = response.interact_pointer_pos() {
            let chosen = |x: f32| {
                markers
                    .clone()
                    .filter_map(|m| {
                        let mark_x = x_at(self.marker_position(m));
                        let distance = (mark_x - x).abs();
                        (distance <= 18.0 && mark_x >= rect.left() && mark_x <= rect.right())
                            .then_some((m, distance))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(m, _)| m)
            };
            if response.clicked() {
                self.marker = chosen(pos.x);
                self.head = frame_at(pos.x);
            } else if response.drag_started() {
                let origin_x = ui.input(|i| i.pointer.press_origin().map_or(pos.x, |p| p.x));
                self.marker = chosen(origin_x);
                self.drag = Some((self.marker, window.start, origin_x));
            }
            if response.dragged() {
                if let Some((marker, start, origin)) = self.drag {
                    if let Some(marker) = marker {
                        self.move_marker(marker, frame_at(pos.x), length);
                    } else {
                        let change = ((origin - pos.x) / rect.width() * window.len() as f32).round()
                            as isize;
                        let start = start
                            .saturating_add_signed(change)
                            .min(length - window.len());
                        self.window = start..start + window.len();
                    }
                }
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
    }
}

#[cfg(test)]
impl VisualSampler {
    pub fn four_hit_fixture() -> Self {
        let pcm = [
            -12_000, -300, 2, 15_000, 0, 5, -2_000, 1, 22_000, 2, -7, -24_000, 4, 3, 9_700, 1,
        ];
        let rate = 48_000u32;
        let len = pcm.len() as u32 * 2;
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((36 + len).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes()); // PCM
        bytes.extend(1u16.to_le_bytes()); // Mono
        bytes.extend(rate.to_le_bytes());
        bytes.extend((rate * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(len.to_le_bytes());
        for frame in pcm {
            bytes.extend((frame as i16).to_le_bytes());
        }
        let (name, rate, decoded) = wav::decode_kit_wav("fixture.wav", &bytes).unwrap();
        assert_eq!(decoded.as_slice(), pcm.map(|sample| sample as i16));
        assert!(wav::decode_kit_wav("invalid.wav", &bytes[..12]).is_err());
        let sample = KitSample::new(name, rate, decoded).unwrap();
        Self {
            source: Some(Source::new(sample)),
            start: 0,
            end: pcm.len(),
            splits: vec![4, 8, 12],
            window: 0..pcm.len(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_max_cache_does_not_read_outside_a_visible_bucket() {
        let pcm: Vec<_> = (0..127).map(|n| ((n * 17) % 83) as i16 - 40).collect();
        let source = Source::new(KitSample::new("signed".into(), 48_000, pcm.clone()).unwrap());
        for start in 0..pcm.len() {
            for end in start + 1..=pcm.len() {
                let slice = &pcm[start..end];
                assert_eq!(
                    source.peak(start..end),
                    (*slice.iter().min().unwrap(), *slice.iter().max().unwrap())
                );
            }
        }
    }

    #[test]
    fn trim_and_splits_enforce_half_open_ranges_and_minimum_length() {
        let mut editor = VisualSampler::four_hit_fixture();
        editor.move_marker(Marker::Start, 8, 16);
        editor.move_marker(Marker::Split(1), 16, 16);
        editor.move_marker(Marker::End, 0, 16);
        editor.head = 3;
        assert!(editor.add_split().is_err());
        assert_eq!(
            editor.ranges().collect::<Vec<_>>(),
            [2..4, 4..10, 10..12, 12..14]
        );
        let kit = editor.make_kit(&Kit::default(), 1).unwrap();
        assert_eq!(kit.samples[1].as_ref().unwrap().pcm, [2, 15_000]);
        assert_eq!(kit.samples[4].as_ref().unwrap().pcm, [4, 3]);
        assert!(editor.make_kit(&kit, 0).is_err());
        assert!(editor.make_kit(&kit, 5).is_err());
    }
}

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use leviathan_core::SongPlayer;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rtrb::{Consumer, Producer, PushError, RingBuffer};

const MIDI_QUEUE_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiKey {
    pub channel: u8,
    pub note: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiEvent {
    NoteOn { key: MidiKey, velocity: u8 },
    NoteOff(MidiKey),
    Start,
    Continue,
    Stop,
}

impl MidiEvent {
    pub fn apply_transport(self, player: &mut SongPlayer, pattern_count: usize) -> bool {
        match self {
            Self::Start => {
                player.reset_transport(pattern_count);
                player.evaluator.playing = true;
            }
            Self::Continue => player.evaluator.playing = true,
            Self::Stop => player.evaluator.playing = false,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
fn parse_message(bytes: &[u8]) -> Option<MidiEvent> {
    match bytes {
        &[status @ 0x80..=0x9f, note @ 0..=127, velocity @ 0..=127] => {
            let key = MidiKey {
                channel: status & 0x0f,
                note,
            };
            if status < 0x90 || velocity == 0 {
                Some(MidiEvent::NoteOff(key))
            } else {
                Some(MidiEvent::NoteOn { key, velocity })
            }
        }
        [0xfa] => Some(MidiEvent::Start),
        [0xfb] => Some(MidiEvent::Continue),
        [0xfc] => Some(MidiEvent::Stop),
        _ => None,
    }
}

fn parse_packet(bytes: &[u8], mut on_event: impl FnMut(MidiEvent)) {
    let mut status: Option<u8> = None;
    let mut data_0: Option<u8> = None;
    let mut in_sysex = false;

    for &byte in bytes {
        match byte {
            0xfa => on_event(MidiEvent::Start),
            0xfb => on_event(MidiEvent::Continue),
            0xfc => on_event(MidiEvent::Stop),
            0xf8 | 0xf9 | 0xfd..=0xff => {}
            0xf0 => {
                in_sysex = true;
                status = None;
                data_0 = None;
            }
            0xf7 => {
                in_sysex = false;
                status = None;
                data_0 = None;
            }
            0xf1..=0xf3 => {
                in_sysex = false;
                status = Some(byte);
                data_0 = None;
            }
            0xf4..=0xf6 => {
                in_sysex = false;
                status = None;
                data_0 = None;
            }
            0x80..=0xef => {
                in_sysex = false;
                status = Some(byte);
                data_0 = None;
            }
            0x00..=0x7f => {
                if in_sysex {
                    continue;
                }
                let Some(st) = status else {
                    continue;
                };
                match st {
                    0xc0..=0xdf => {
                        data_0 = None;
                    }
                    0xf1 | 0xf3 => {
                        status = None;
                        data_0 = None;
                    }
                    0xf2 => {
                        if data_0.take().is_none() {
                            data_0 = Some(byte);
                        } else {
                            status = None;
                        }
                    }
                    _ => {
                        if let Some(first) = data_0.take() {
                            if (0x80..=0x9f).contains(&st) {
                                let key = MidiKey {
                                    channel: st & 0x0f,
                                    note: first,
                                };
                                if st < 0x90 || byte == 0 {
                                    on_event(MidiEvent::NoteOff(key));
                                } else {
                                    on_event(MidiEvent::NoteOn {
                                        key,
                                        velocity: byte,
                                    });
                                }
                            }
                        } else {
                            data_0 = Some(byte);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MidiTarget {
    pub pattern: usize,
    pub lane: usize,
    pub row: usize,
}

#[derive(Clone, Copy)]
struct MidiPacket {
    port_epoch: u64,
    event: MidiEvent,
}

struct MidiSender {
    queue: Producer<MidiPacket>,
    overflow: Arc<AtomicBool>,
}

impl MidiSender {
    fn push(&mut self, packet: MidiPacket) {
        if !self.queue.is_abandoned() && self.queue.push(packet).is_err() {
            self.overflow.store(true, Ordering::Release);
        }
    }
}

pub struct MidiReceiver {
    queue: Consumer<MidiPacket>,
    overflow: Arc<AtomicBool>,
    port_epoch: Arc<AtomicU64>,
    current_epoch: u64,
}

impl MidiReceiver {
    fn channel(port_epoch: &Arc<AtomicU64>) -> (MidiSender, Self) {
        let (tx, rx) = RingBuffer::new(MIDI_QUEUE_CAPACITY);
        let overflow = Arc::new(AtomicBool::new(false));
        (
            MidiSender {
                queue: tx,
                overflow: Arc::clone(&overflow),
            },
            Self {
                queue: rx,
                overflow,
                port_epoch: Arc::clone(port_epoch),
                current_epoch: 0,
            },
        )
    }

    /// Returns whether held notes should release, and the bounded batch length.
    pub fn begin_batch(&mut self) -> (bool, usize) {
        // Snapshot the queue before the epoch so a new port's events can wait for the next batch.
        let count = self.queue.slots();
        let epoch = self.port_epoch.load(Ordering::Acquire);
        let changed = self.current_epoch != epoch;
        self.current_epoch = epoch;
        if self.overflow.swap(false, Ordering::AcqRel) {
            // A dropped Note Off could otherwise leave an audition gate held.
            self.discard_pending();
            (true, 0)
        } else {
            (changed, count)
        }
    }

    pub fn pop(&mut self) -> Option<MidiEvent> {
        let packet = self.queue.pop().ok()?;
        (packet.port_epoch == self.current_epoch).then_some(packet.event)
    }

    pub fn discard_pending(&mut self) {
        for _ in 0..self.queue.slots() {
            let _ = self.queue.pop();
        }
    }
}

struct MidiSenders {
    audio: MidiSender,
    ui: MidiSender,
}

impl MidiSenders {
    fn push(&mut self, packet: MidiPacket) {
        self.audio.push(packet);
        if !matches!(packet.event, MidiEvent::NoteOff(_)) {
            self.ui.push(packet);
        }
    }
}

// Transfer the permanent senders only after connect succeeds. A failed midir
// connect consumes its callback data, while the idle senders stay with the UI.
struct CallbackData {
    port_epoch: u64,
    handoff: Consumer<MidiSenders>,
    senders: Option<MidiSenders>,
}

fn receive_message(_timestamp: u64, bytes: &[u8], data: &mut CallbackData) {
    parse_packet(bytes, |event| {
        if data.senders.is_none() {
            data.senders = data.handoff.pop().ok();
        }
        if let Some(senders) = &mut data.senders {
            senders.push(MidiPacket {
                port_epoch: data.port_epoch,
                event,
            });
        }
    });
}

struct Port {
    id: String,
    name: String,
}

struct ActivePort {
    id: String,
    connection: MidiInputConnection<CallbackData>,
}

pub struct MidiControls {
    ports: Vec<Port>,
    active: Option<ActivePort>,
    idle: Option<MidiSenders>,
    port_epoch: Arc<AtomicU64>,
    pub events: MidiReceiver,
    pub record_armed: bool,
    error: Option<String>,
}

impl MidiControls {
    pub fn new() -> (Self, MidiReceiver) {
        let port_epoch = Arc::new(AtomicU64::new(0));
        let (audio, audio_events) = MidiReceiver::channel(&port_epoch);
        let (ui, ui_events) = MidiReceiver::channel(&port_epoch);
        (
            Self {
                ports: Vec::new(),
                active: None,
                idle: Some(MidiSenders { audio, ui }),
                port_epoch,
                events: ui_events,
                record_armed: false,
                error: None,
            },
            audio_events,
        )
    }

    pub fn refresh(&mut self) {
        let ports = MidiInput::new("Leviathan").map(|input| {
            input
                .ports()
                .into_iter()
                .filter_map(|port| {
                    input.port_name(&port).ok().map(|name| Port {
                        id: port.id(),
                        name,
                    })
                })
                .collect()
        });
        self.ports = ports.unwrap_or_default();
        self.error = None;
        if self
            .active
            .as_ref()
            .is_some_and(|active| !self.ports.iter().any(|port| port.id == active.id))
        {
            self.disconnect();
        }
    }

    fn close_port(&mut self) {
        if let Some(active) = self.active.take() {
            let (_, mut data) = active.connection.close();
            self.idle = data.senders.take().or_else(|| data.handoff.pop().ok());
        }
    }

    fn disconnect(&mut self) {
        self.close_port();
        self.port_epoch.fetch_add(1, Ordering::Release);
    }

    fn connect(&mut self, id: String) -> Result<(), String> {
        let mut input = MidiInput::new("Leviathan").map_err(|error| error.to_string())?;
        input.ignore(Ignore::None);
        let port = input
            .find_port_by_id(&id)
            .ok_or_else(|| "MIDI port disappeared. Refresh the port list.".to_string())?;
        let epoch = self.port_epoch.load(Ordering::Relaxed).wrapping_add(1);
        let (mut handoff, receive) = RingBuffer::new(1);
        let connection = input
            .connect(
                &port,
                "Leviathan Input",
                receive_message,
                CallbackData {
                    port_epoch: epoch,
                    handoff: receive,
                    senders: None,
                },
            )
            .map_err(|error| error.to_string())?;

        self.close_port();
        let senders = self
            .idle
            .take()
            .ok_or_else(|| "MIDI input queue is unavailable.".to_string())?;
        self.port_epoch.store(epoch, Ordering::Release);
        if let Err(PushError::Full(senders)) = handoff.push(senders) {
            self.idle = Some(senders);
            return Err("MIDI input queue could not be attached".to_string());
        }
        self.active = Some(ActivePort { id, connection });
        Ok(())
    }

    fn label(&self) -> String {
        if let Some(name) = self.active.as_ref().and_then(|active| {
            self.ports
                .iter()
                .find(|port| port.id == active.id)
                .map(|port| port.name.as_str())
        }) {
            format!("MIDI: {name}")
        } else if self.ports.is_empty() {
            "MIDI: No ports".to_string()
        } else {
            "MIDI: Off".to_string()
        }
    }

    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let selected = self.active.as_ref().map(|active| active.id.clone());
        let mut next = selected.clone();
        egui::ComboBox::from_id_salt("midi_input")
            .selected_text(self.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut next, None, "Off");
                for port in &self.ports {
                    ui.selectable_value(&mut next, Some(port.id.clone()), &port.name);
                }
            });
        if next != selected {
            self.error = match next {
                Some(id) => self.connect(id).err(),
                None => {
                    self.disconnect();
                    None
                }
            };
        }
        if ui.button("Refresh MIDI").clicked() {
            self.refresh();
        }
        ui.toggle_value(&mut self.record_armed, "MIDI REC")
            .on_hover_text("Write MIDI notes and velocities at the tracker cursor");
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.active.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    pub fn inject(&mut self, event: MidiEvent) {
        self.idle.as_mut().unwrap().push(MidiPacket {
            port_epoch: self.port_epoch.load(Ordering::Relaxed),
            event,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_notes_velocity_zero_and_transport() {
        let key = MidiKey {
            channel: 15,
            note: 127,
        };
        assert_eq!(
            parse_message(&[0x9f, 127, 1]),
            Some(MidiEvent::NoteOn { key, velocity: 1 })
        );
        assert_eq!(
            parse_message(&[0x9f, 127, 0]),
            Some(MidiEvent::NoteOff(key))
        );
        assert_eq!(
            parse_message(&[0x8f, 127, 64]),
            Some(MidiEvent::NoteOff(key))
        );
        for (byte, event) in [
            (0xfa, MidiEvent::Start),
            (0xfb, MidiEvent::Continue),
            (0xfc, MidiEvent::Stop),
        ] {
            assert_eq!(parse_message(&[byte]), Some(event));
        }
        for bytes in [
            &[][..],
            &[0x90, 60],
            &[0x90, 128, 4],
            &[0x90, 60, 255],
            &[0xfa, 0],
            &[0xf0, 1, 2, 0xf7],
            &[0xf8],
            &[0xb0, 1, 1],
        ] {
            assert_eq!(parse_message(bytes), None);
        }
    }

    #[test]
    fn packet_framer_unpacks_chords_running_status_and_interleaved_realtime() {
        let mut chord = Vec::new();
        parse_packet(&[0x92, 60, 100, 0x92, 64, 96, 0x92, 67, 92], |ev| {
            chord.push(ev)
        });
        assert_eq!(
            chord,
            vec![
                MidiEvent::NoteOn {
                    key: MidiKey {
                        channel: 2,
                        note: 60
                    },
                    velocity: 100,
                },
                MidiEvent::NoteOn {
                    key: MidiKey {
                        channel: 2,
                        note: 64
                    },
                    velocity: 96,
                },
                MidiEvent::NoteOn {
                    key: MidiKey {
                        channel: 2,
                        note: 67
                    },
                    velocity: 92,
                },
            ]
        );

        let mut mixed = Vec::new();
        parse_packet(
            &[
                0xfa, 0xb1, 1, 64, 0x91, 48, 0xf8, 110, 52, 95, 48, 0, 0xf0, 0x7e, 0x7f, 0xf7,
                60, 100, 0xfc,
            ],
            |ev| mixed.push(ev),
        );
        assert_eq!(
            mixed,
            vec![
                MidiEvent::Start,
                MidiEvent::NoteOn {
                    key: MidiKey {
                        channel: 1,
                        note: 48
                    },
                    velocity: 110,
                },
                MidiEvent::NoteOn {
                    key: MidiKey {
                        channel: 1,
                        note: 52
                    },
                    velocity: 95,
                },
                MidiEvent::NoteOff(MidiKey {
                    channel: 1,
                    note: 48
                }),
                MidiEvent::Stop,
            ]
        );

        let epoch = Arc::new(AtomicU64::new(7));
        let (audio_tx, mut audio_rx) = MidiReceiver::channel(&epoch);
        let (ui_tx, mut ui_rx) = MidiReceiver::channel(&epoch);
        let (mut handoff_tx, handoff_rx) = RingBuffer::new(1);
        handoff_tx
            .push(MidiSenders {
                audio: audio_tx,
                ui: ui_tx,
            })
            .unwrap();
        let mut cb_data = CallbackData {
            port_epoch: 7,
            handoff: handoff_rx,
            senders: None,
        };

        receive_message(
            0,
            &[0x90, 60, 100, 0x90, 64, 90, 0x80, 60, 0],
            &mut cb_data,
        );

        let (_, audio_n) = audio_rx.begin_batch();
        assert_eq!(audio_n, 3);
        assert_eq!(
            audio_rx.pop(),
            Some(MidiEvent::NoteOn {
                key: MidiKey {
                    channel: 0,
                    note: 60
                },
                velocity: 100,
            })
        );
        assert_eq!(
            audio_rx.pop(),
            Some(MidiEvent::NoteOn {
                key: MidiKey {
                    channel: 0,
                    note: 64
                },
                velocity: 90,
            })
        );
        assert_eq!(
            audio_rx.pop(),
            Some(MidiEvent::NoteOff(MidiKey {
                channel: 0,
                note: 60
            }))
        );

        let (_, ui_n) = ui_rx.begin_batch();
        assert_eq!(ui_n, 2);
    }

    #[test]
    fn headless_enumeration_has_a_no_ports_fallback() {
        let (mut controls, _) = MidiControls::new();
        assert_eq!(controls.label(), "MIDI: No ports");
        controls.refresh();
        if controls.ports.is_empty() {
            assert_eq!(controls.label(), "MIDI: No ports");
        } else {
            assert_eq!(controls.label(), "MIDI: Off");
        }
    }

    #[test]
    fn audio_queue_resets_on_overflow_and_skips_old_ports() {
        let (mut controls, mut audio) = MidiControls::new();
        for _ in 0..=MIDI_QUEUE_CAPACITY {
            controls.inject(MidiEvent::Start);
        }
        assert_eq!(audio.begin_batch(), (true, 0));
        assert_eq!(audio.pop(), None);

        controls.inject(MidiEvent::Stop);
        controls.disconnect();
        controls.inject(MidiEvent::Continue);
        assert_eq!(audio.begin_batch(), (true, 2));
        assert_eq!(audio.pop(), None);
        assert_eq!(audio.pop(), Some(MidiEvent::Continue));
    }
}

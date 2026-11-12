//! Standard MIDI File interchange (RP-001). See https://midi.org/standard-midi-files.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use leviathan_core::{
    NoteCell, Pattern, PatternChain, PlaybackRate, MAX_CHAIN_ENTRIES, NUM_LANES, ROWS_PER_PATTERN,
};

use crate::project::PATTERN_BANK_CAPACITY;

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const EXPORT_PPQN: u16 = 960;
const DEFAULT_TEMPO: u32 = 500_000;
const MICROS_PER_MINUTE: f64 = 60_000_000.0;
// Standard marker and text events carry tracker data that ordinary MIDI notes cannot represent.
const GRID_MARKER: &str = "Leviathan pattern ";
const BPM_TEXT: &str = "Leviathan BPM ";
const AIR_TEXT: &[u8] = b"Leviathan note air";
const ZERO_TEXT: &[u8] = b"Leviathan note zero";
const AIR_ZERO_TEXT: &[u8] = b"Leviathan note air zero";

pub const GRID_RATES: [PlaybackRate; 6] = [
    PlaybackRate::Quarter,
    PlaybackRate::Half,
    PlaybackRate::OneX,
    PlaybackRate::TwoX,
    PlaybackRate::FourX,
    PlaybackRate::EightX,
];

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Pattern,
    Chain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    SingleTrack,
    #[default]
    MultiTrack,
}

pub struct Import {
    pub patterns: Vec<Pattern>,
    pub chain: PatternChain,
    pub bpm: f64,
    pub dropped_notes: usize,
    pub tempo_changes: bool,
}

pub fn load(path: &Path, rate: PlaybackRate, scope: Scope) -> Result<Import> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    decode(&bytes, rate, scope)
}

pub fn save(path: &Path, patterns: &[&Pattern], bpm: f64, format: Format) -> Result<()> {
    let bytes = encode(patterns, bpm, format)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(())
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let (head, tail) = self
            .0
            .split_at_checked(count)
            .ok_or("Truncated MIDI file")?;
        self.0 = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn data_byte(&mut self) -> Result<u8> {
        let byte = self.byte()?;
        if byte < 0x80 {
            Ok(byte)
        } else {
            Err("Expected a MIDI data byte".into())
        }
    }

    fn vlq(&mut self) -> Result<u32> {
        let mut value = 0_u32;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("MIDI variable-length quantity exceeds four bytes".into())
    }

    fn chunk(&mut self) -> Result<(&'a [u8], &'a [u8])> {
        let name = self.take(4)?;
        let length = u32::from_be_bytes(self.take(4)?.try_into().unwrap());
        Ok((name, self.take(length as usize)?))
    }
}

struct Note {
    tick: u64,
    end: Option<u64>,
    track: u16,
    channel: u8,
    pitch: u8,
    velocity: u8,
    air: bool,
}

#[derive(Clone, Copy)]
struct Grid {
    tick: u64,
    rows: usize,
    rate: PlaybackRate,
}

#[derive(Default)]
struct Timeline {
    notes: Vec<Note>,
    grid: Vec<Grid>,
    end_tick: u64,
    tempo: Option<u32>,
    exact_bpm: Option<f64>,
    tempo_changes: bool,
}

impl Timeline {
    fn read_meta(&mut self, tick: u64, kind: u8, data: &[u8]) -> Result<()> {
        match kind {
            0x51 => {
                let [a, b, c] = data else {
                    return Err("MIDI tempo must contain three bytes".into());
                };
                let tempo = u32::from_be_bytes([0, *a, *b, *c]);
                if tempo == 0 {
                    return Err("MIDI tempo must be positive".into());
                }
                if tick == 0 && self.tempo.is_none() {
                    self.tempo = Some(tempo);
                } else {
                    self.tempo_changes = true;
                }
            }
            0x01 if tick == 0 => {
                if let Some(text) = std::str::from_utf8(data)
                    .ok()
                    .and_then(|text| text.strip_prefix(BPM_TEXT))
                {
                    self.exact_bpm = text.parse().ok();
                }
            }
            0x06 if self.grid.len() <= MAX_CHAIN_ENTRIES => {
                if let Some(grid) = parse_grid(tick, data) {
                    self.grid.push(grid);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn bpm(&self) -> f64 {
        let tempo = self.tempo.unwrap_or(DEFAULT_TEMPO);
        self.exact_bpm
            .filter(|bpm| (40.0..=240.0).contains(bpm) && midi_tempo(*bpm) == tempo)
            .unwrap_or(MICROS_PER_MINUTE / f64::from(tempo))
    }

    fn native_grid(&mut self, ppqn: u16) -> bool {
        self.grid.sort_by_key(|grid| grid.tick);
        let mut end = 0;
        for grid in &self.grid {
            let units = grid.rows as u64 * u64::from(ppqn);
            let rate = rows_per_beat(grid.rate);
            if grid.tick != end || units % rate != 0 {
                return false;
            }
            end += units / rate;
        }
        !self.grid.is_empty()
            && self.grid.len() <= MAX_CHAIN_ENTRIES
            && end == self.end_tick
            && self.notes.iter().all(|note| note.tick < end)
    }
}

fn rows_per_beat(rate: PlaybackRate) -> u64 {
    rate.rows_per_beat() as u64
}

fn parse_grid(tick: u64, data: &[u8]) -> Option<Grid> {
    let mut fields = std::str::from_utf8(data)
        .ok()?
        .strip_prefix(GRID_MARKER)?
        .split_ascii_whitespace();
    let rows = fields.next()?.parse().ok()?;
    let resolution: u64 = fields.next()?.parse().ok()?;
    let rate = GRID_RATES
        .into_iter()
        .find(|&rate| rows_per_beat(rate) == resolution)?;
    ((1..=ROWS_PER_PATTERN).contains(&rows) && fields.next().is_none()).then_some(Grid {
        tick,
        rows,
        rate,
    })
}

fn read_track(bytes: &[u8], track: u16, timeline: &mut Timeline) -> Result<()> {
    let mut reader = Reader(bytes);
    let mut tick = 0_u64;
    let mut status = None;
    let mut flags = None;
    let mut active = BTreeMap::<(u8, u8), VecDeque<usize>>::new();
    while !reader.0.is_empty() {
        tick += u64::from(reader.vlq()?);
        let byte = reader.byte()?;
        let cell_flags = flags.take();
        match byte {
            0xff => {
                status = None;
                let kind = reader.data_byte()?;
                let length = reader.vlq()? as usize;
                let data = reader.take(length)?;
                if kind == 0x2f {
                    if length != 0 || !reader.0.is_empty() {
                        return Err("Invalid MIDI end-of-track event".into());
                    }
                    timeline.end_tick = timeline.end_tick.max(tick);
                    return Ok(());
                }
                if kind == 0x01 {
                    flags = match data {
                        AIR_TEXT => Some((tick, true, false)),
                        ZERO_TEXT => Some((tick, false, true)),
                        AIR_ZERO_TEXT => Some((tick, true, true)),
                        _ => None,
                    };
                }
                timeline.read_meta(tick, kind, data)?;
            }
            0xf0 | 0xf7 => {
                status = None;
                let length = reader.vlq()? as usize;
                reader.take(length)?;
            }
            0xf8..=0xfe => {
                flags = cell_flags;
            }
            0x00..=0xef => {
                let (message, pitch) = if byte >= 0x80 {
                    status = Some(byte);
                    (byte, reader.data_byte()?)
                } else {
                    (
                        status.ok_or("MIDI running status has no channel message")?,
                        byte,
                    )
                };
                let kind = message & 0xf0;
                if matches!(kind, 0xc0 | 0xd0) {
                    continue;
                }
                let value = reader.data_byte()?;
                let channel = message & 0x0f;
                if kind == 0x90 && value != 0 {
                    let (_, air, zero) = cell_flags
                        .filter(|&(flag_tick, _, _)| flag_tick == tick)
                        .unwrap_or((0, false, false));
                    active
                        .entry((channel, pitch))
                        .or_default()
                        .push_back(timeline.notes.len());
                    timeline.notes.push(Note {
                        tick,
                        end: None,
                        track,
                        channel,
                        pitch,
                        velocity: if zero && value == 1 { 0 } else { value },
                        air,
                    });
                } else if kind == 0x80 || kind == 0x90 {
                    if let Some(index) = active
                        .get_mut(&(channel, pitch))
                        .and_then(VecDeque::pop_front)
                    {
                        timeline.notes[index].end = Some(tick);
                    }
                }
            }
            _ => return Err("Unsupported system status in MIDI track".into()),
        }
    }
    Err("MIDI track is missing its end-of-track event".into())
}

fn quantized_row(tick: u64, rate: PlaybackRate, ppqn: u16) -> u64 {
    (tick * rows_per_beat(rate) + u64::from(ppqn) / 2) / u64::from(ppqn)
}

fn read_timeline(bytes: &[u8]) -> Result<(Timeline, u16)> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err("MIDI file exceeds 2 MiB".into());
    }
    let mut input = Reader(bytes);
    let (name, header) = input.chunk()?;
    if name != b"MThd" || header.len() < 6 {
        return Err("Invalid MIDI header".into());
    }
    let format = u16::from_be_bytes([header[0], header[1]]);
    let track_count = u16::from_be_bytes([header[2], header[3]]);
    let ppqn = u16::from_be_bytes([header[4], header[5]]);
    if format > 1 || track_count == 0 || (format == 0 && track_count != 1) {
        return Err("Expected a format 0 or format 1 MIDI file with valid track count".into());
    }
    if ppqn == 0 || ppqn & 0x8000 != 0 {
        return Err("MIDI import requires PPQN timing".into());
    }
    let mut timeline = Timeline::default();
    let mut track = 0;
    while !input.0.is_empty() {
        let (name, bytes) = input.chunk()?;
        if name == b"MTrk" {
            if track == track_count {
                return Err("MIDI file contains more tracks than its header declares".into());
            }
            read_track(bytes, track, &mut timeline)?;
            track += 1;
        }
    }
    if track != track_count {
        return Err("MIDI file is missing a declared track".into());
    }
    Ok((timeline, ppqn))
}

fn decode(bytes: &[u8], rate: PlaybackRate, scope: Scope) -> Result<Import> {
    let (mut timeline, ppqn) = read_timeline(bytes)?;
    let native = timeline.native_grid(ppqn);
    let mut patterns = if native {
        timeline
            .grid
            .iter()
            .map(|grid| Pattern {
                length: grid.rows,
                rate: grid.rate,
                ..Pattern::default()
            })
            .collect::<Vec<_>>()
    } else {
        let end = (timeline.end_tick * rows_per_beat(rate)).div_ceil(u64::from(ppqn));
        let last_note = timeline
            .notes
            .iter()
            .map(|note| quantized_row(note.tick, rate, ppqn) + 1)
            .max()
            .unwrap_or(1);
        let rows = end.max(last_note);
        if rows > (MAX_CHAIN_ENTRIES * ROWS_PER_PATTERN) as u64 {
            return Err("MIDI file exceeds the song chain row limit at this grid rate".into());
        }
        (0..rows as usize)
            .step_by(ROWS_PER_PATTERN)
            .map(|start| Pattern {
                length: (rows as usize - start).min(ROWS_PER_PATTERN),
                rate,
                ..Pattern::default()
            })
            .collect()
    };
    if scope == Scope::Pattern && patterns.len() != 1 {
        return Err("MIDI file needs more than one pattern. Import it as a chain.".into());
    }

    let mut preferred_lanes = BTreeMap::new();
    let mut used_lanes = [false; NUM_LANES];
    for note in &timeline.notes {
        preferred_lanes
            .entry((note.track, note.channel))
            .or_insert_with(|| {
                let channel = usize::from(note.channel);
                let lane = if channel < NUM_LANES && !used_lanes[channel] {
                    channel
                } else {
                    used_lanes
                        .iter()
                        .position(|used| !used)
                        .unwrap_or(channel % NUM_LANES)
                };
                used_lanes[lane] = true;
                lane
            });
    }
    let bpm = timeline.bpm();
    timeline.notes.sort_by_key(|note| note.tick);
    let mut dropped_notes = 0;
    for note in timeline.notes {
        let (page, row) = if native {
            let page = timeline.grid.partition_point(|grid| grid.tick <= note.tick) - 1;
            let grid = timeline.grid[page];
            let row = quantized_row(note.tick - grid.tick, grid.rate, ppqn) as usize;
            (page, row.min(grid.rows - 1))
        } else {
            let row = quantized_row(note.tick, rate, ppqn) as usize;
            (row / ROWS_PER_PATTERN, row % ROWS_PER_PATTERN)
        };
        let pattern = &mut patterns[page];
        let preferred = preferred_lanes[&(note.track, note.channel)];
        let lane = if !pattern.lanes[preferred][row].active {
            Some(preferred)
        } else {
            pattern.lanes.iter().position(|lane| !lane[row].active)
        };
        if let Some(lane) = lane {
            let gate_fraction = note.end.map_or(0.5, |end| {
                ((end - note.tick) as f64 * pattern.rate.rows_per_beat() / f64::from(ppqn))
                    .clamp(0.0, 1.0) as f32
            });
            pattern.lanes[lane][row] = NoteCell {
                active: true,
                air: note.air,
                midi: note.pitch,
                velocity: note.velocity,
                gate_fraction,
                ..NoteCell::default()
            };
        } else {
            dropped_notes += 1;
        }
    }

    let mut unique = Vec::<Pattern>::new();
    let mut chain = PatternChain::default();
    for pattern in patterns {
        let index = unique.iter().position(|existing| {
            existing.length == pattern.length
                && existing.rate == pattern.rate
                && existing.lanes == pattern.lanes
        });
        let index = match index {
            Some(index) => index,
            None if unique.len() < PATTERN_BANK_CAPACITY => {
                let index = unique.len();
                unique.push(pattern);
                index
            }
            None => return Err("MIDI chain exceeds the pattern bank capacity".into()),
        };
        chain.push(index as u8);
    }
    Ok(Import {
        patterns: unique,
        chain,
        bpm,
        dropped_notes,
        tempo_changes: timeline.tempo_changes,
    })
}

enum Message {
    Tempo(u32),
    Text(String),
    Marker(String),
    On { lane: u8, cell: NoteCell },
    Off { lane: u8, pitch: u8 },
}

impl Message {
    fn order(&self) -> u8 {
        match self {
            Self::Off { .. } => 0,
            Self::On { .. } => 2,
            _ => 1,
        }
    }
}

fn midi_tempo(bpm: f64) -> u32 {
    (MICROS_PER_MINUTE / bpm).round() as u32
}

fn encode(patterns: &[&Pattern], bpm: f64, format: Format) -> Result<Vec<u8>> {
    if patterns.is_empty() || patterns.len() > MAX_CHAIN_ENTRIES {
        return Err("Select a pattern or a nonempty chain to export".into());
    }
    if !(40.0..=240.0).contains(&bpm) {
        return Err("MIDI export requires a BPM between 40 and 240".into());
    }
    let mut conductor = vec![
        (0, Message::Tempo(midi_tempo(bpm))),
        (0, Message::Text(format!("{BPM_TEXT}{bpm}"))),
    ];
    let mut lanes = std::array::from_fn::<_, NUM_LANES, _>(|_| Vec::new());
    let mut tick = 0;
    for pattern in patterns {
        let length = pattern.length.clamp(1, ROWS_PER_PATTERN);
        let resolution = rows_per_beat(pattern.rate);
        let row_ticks = u64::from(EXPORT_PPQN) / resolution;
        conductor.push((
            tick,
            Message::Marker(format!("{GRID_MARKER}{length} {resolution}")),
        ));
        for (lane, events) in lanes.iter_mut().enumerate() {
            for (row, &cell) in pattern.lanes[lane][..length].iter().enumerate() {
                if !cell.active {
                    continue;
                }
                let lane = lane as u8;
                let start = tick + row as u64 * row_ticks;
                let gate = (f64::from(cell.gate_fraction) * row_ticks as f64).round() as u64;
                events.push((start, Message::On { lane, cell }));
                events.push((
                    start + gate.clamp(1, row_ticks),
                    Message::Off {
                        lane,
                        pitch: cell.midi,
                    },
                ));
            }
        }
        tick += length as u64 * row_ticks;
    }

    let mut output = b"MThd\0\0\0\x06".to_vec();
    let (format_id, tracks) = match format {
        Format::SingleTrack => (0_u16, 1_u16),
        Format::MultiTrack => (1, (NUM_LANES + 1) as u16),
    };
    output.extend(format_id.to_be_bytes());
    output.extend(tracks.to_be_bytes());
    output.extend(EXPORT_PPQN.to_be_bytes());
    match format {
        Format::SingleTrack => {
            conductor.extend(lanes.into_iter().flatten());
            write_track(&mut output, conductor, tick, None);
        }
        Format::MultiTrack => {
            write_track(&mut output, conductor, tick, Some("Leviathan tempo"));
            for (lane, events) in lanes.into_iter().enumerate() {
                write_track(
                    &mut output,
                    events,
                    tick,
                    Some(&format!("Lane {}", lane + 1)),
                );
            }
        }
    }
    Ok(output)
}

fn write_track(
    output: &mut Vec<u8>,
    mut events: Vec<(u64, Message)>,
    end: u64,
    name: Option<&str>,
) {
    events.sort_by_key(|(tick, event)| (*tick, event.order()));
    let mut track = Vec::new();
    if let Some(name) = name {
        track.push(0);
        write_meta(&mut track, 0x03, name.as_bytes());
    }
    let mut previous = 0;
    for (tick, event) in events {
        write_vlq(&mut track, tick - previous);
        previous = tick;
        match event {
            Message::Tempo(tempo) => write_meta(&mut track, 0x51, &tempo.to_be_bytes()[1..]),
            Message::Text(text) => write_meta(&mut track, 0x01, text.as_bytes()),
            Message::Marker(text) => write_meta(&mut track, 0x06, text.as_bytes()),
            Message::Off { lane, pitch } => track.extend([0x80 | lane, pitch.min(127), 0]),
            Message::On { lane, cell } => {
                let annotation = match (cell.air, cell.velocity == 0) {
                    (true, true) => Some(AIR_ZERO_TEXT),
                    (true, false) => Some(AIR_TEXT),
                    (false, true) => Some(ZERO_TEXT),
                    (false, false) => None,
                };
                if let Some(text) = annotation {
                    write_meta(&mut track, 0x01, text);
                    track.push(0);
                }
                track.extend([0x90 | lane, cell.midi.min(127), cell.velocity.clamp(1, 127)]);
            }
        }
    }
    write_vlq(&mut track, end - previous);
    write_meta(&mut track, 0x2f, &[]);
    output.extend(b"MTrk");
    output.extend((track.len() as u32).to_be_bytes());
    output.extend(track);
}

fn write_meta(track: &mut Vec<u8>, kind: u8, bytes: &[u8]) {
    track.extend([0xff, kind]);
    write_vlq(track, bytes.len() as u64);
    track.extend(bytes);
}

fn write_vlq(bytes: &mut Vec<u8>, mut value: u64) {
    let mut buffer = [0_u8; 4];
    let mut first = buffer.len() - 1;
    buffer[first] = (value & 0x7f) as u8;
    while value > 0x7f {
        value >>= 7;
        first -= 1;
        buffer[first] = 0x80 | (value & 0x7f) as u8;
    }
    bytes.extend(&buffer[first..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(format: u16, ppqn: u16, tracks: &[&[u8]]) -> Vec<u8> {
        let mut file = b"MThd\x00\x00\x00\x06".to_vec();
        file.extend(format.to_be_bytes());
        file.extend((tracks.len() as u16).to_be_bytes());
        file.extend(ppqn.to_be_bytes());
        for track in tracks {
            file.extend(b"MTrk");
            file.extend((track.len() as u32).to_be_bytes());
            file.extend(*track);
        }
        file
    }

    fn note(midi: u8, velocity: u8) -> NoteCell {
        NoteCell {
            active: true,
            midi,
            velocity,
            ..NoteCell::default()
        }
    }

    #[test]
    fn both_formats_restore_the_grid_lanes_and_exact_tempo() {
        let mut pattern = Pattern {
            length: 13,
            rate: PlaybackRate::EightX,
            ..Pattern::default()
        };
        pattern.lanes[2][0] = note(0, 127);
        pattern.lanes[5][1] = note(127, 1);
        pattern.lanes[2][12] = NoteCell {
            air: true,
            ..note(60, 0)
        };
        let bpm = 132.123_456_789;
        for (format, header) in [
            (Format::SingleTrack, [0, 0, 0, 1]),
            (Format::MultiTrack, [0, 1, 0, 7]),
        ] {
            let bytes = encode(&[&pattern], bpm, format).unwrap();
            assert_eq!(bytes[8..12], header);
            let import = decode(&bytes, PlaybackRate::OneX, Scope::Pattern).unwrap();
            assert_eq!(import.bpm.to_bits(), bpm.to_bits());
            assert_eq!(import.chain.as_slice(), &[0]);
            assert_eq!(import.dropped_notes, 0);
            let loaded = &import.patterns[0];
            assert_eq!((loaded.length, loaded.rate), (pattern.length, pattern.rate));
            assert_eq!(loaded.lanes, pattern.lanes);
        }
    }

    #[test]
    fn song_chain_keeps_mixed_rates_trailing_rests_and_repeats() {
        let mut first = Pattern {
            length: 3,
            rate: PlaybackRate::Half,
            ..Pattern::default()
        };
        first.lanes[4][0] = note(47, 112);
        let mut second = Pattern {
            length: 17,
            rate: PlaybackRate::FourX,
            ..Pattern::default()
        };
        second.lanes[1][15] = note(81, 72);
        let bytes = encode(&[&first, &second, &first], 120.0, Format::MultiTrack).unwrap();
        assert!(decode(&bytes, PlaybackRate::OneX, Scope::Pattern).is_err());
        let import = decode(&bytes, PlaybackRate::OneX, Scope::Chain).unwrap();
        assert_eq!(import.patterns.len(), 2);
        assert_eq!(import.chain.as_slice(), &[0, 1, 0]);
        for (expected, loaded) in [&first, &second].into_iter().zip(import.patterns) {
            assert_eq!(
                (loaded.length, loaded.rate),
                (expected.length, expected.rate)
            );
            assert_eq!(loaded.lanes, expected.lanes);
        }
    }

    #[test]
    fn reads_running_status_variable_length_deltas_sysex_and_tempo() {
        let track = [
            0, 0xff, 0x51, 3, 0x06, 0x1a, 0x80, // 150 BPM
            0, 0xc0, 5, // program change
            0, 6, // running status, one byte
            0, 0xf0, 3, 0x7d, 0x01, 0xf7, // system exclusive
            0, 0x90, 60, 100, // two simultaneous notes on channel 1
            0, 64, 90, 0x81, 0x70, 60, 0, // running-status note-off at tick 240
            0, 64, 0, 0, 0x91, 67, 127, 60, 0x81, 67, 0, 60, 0xff, 0x51, 3, 0x07, 0xa1, 0x20, 0,
            0xff, 0x2f, 0,
        ];
        let bytes = fixture(0, 480, &[&track]);
        let import = decode(&bytes, PlaybackRate::OneX, Scope::Pattern).unwrap();
        assert_eq!(import.bpm, 150.0);
        assert!(import.tempo_changes);
        let pattern = &import.patterns[0];
        assert_eq!(pattern.length, 3);
        assert_eq!(
            (pattern.lanes[0][0].midi, pattern.lanes[0][0].velocity),
            (60, 100)
        );
        assert_eq!(
            (pattern.lanes[1][0].midi, pattern.lanes[1][0].velocity),
            (64, 90)
        );
        assert_eq!(pattern.lanes[1][2], note(67, 127));
    }

    #[test]
    fn type_one_tracks_using_the_same_channel_get_separate_lanes() {
        let track1 = [0, 0x90, 48, 100, 24, 0x80, 48, 0, 0, 0xff, 0x2f, 0];
        let track2 = [0, 0x90, 72, 80, 24, 0x80, 72, 0, 0, 0xff, 0x2f, 0];
        let bytes = fixture(1, 96, &[&[0, 0xff, 0x2f, 0], &track1, &track2]);
        let import = decode(&bytes, PlaybackRate::OneX, Scope::Pattern).unwrap();
        assert_eq!(import.bpm, 120.0);
        let pattern = &import.patterns[0];
        assert_eq!(pattern.lanes[0][0].midi, 48);
        assert_eq!(pattern.lanes[1][0].midi, 72);
    }

    #[test]
    fn generic_chain_uses_the_chosen_grid_and_pages_at_sixty_four_rows() {
        let track = [
            0, 0x90, 62, 90, 1, 62, 0, 63, 64, 92, 1, 64, 0, 0, 0xff, 0x2f, 0,
        ];
        let bytes = fixture(0, 4, &[&track]);
        assert!(decode(&bytes, PlaybackRate::OneX, Scope::Pattern).is_err());
        let import = decode(&bytes, PlaybackRate::OneX, Scope::Chain).unwrap();
        assert_eq!(import.chain.as_slice(), &[0, 1]);
        assert_eq!(
            (import.patterns[0].length, import.patterns[1].length),
            (64, 1)
        );
        assert_eq!(import.patterns[1].lanes[0][0].midi, 64);
        assert_eq!(import.patterns[1].lanes[0][0].velocity, 92);
    }

    #[test]
    fn reports_chords_that_exceed_the_available_lanes() {
        let mut track = vec![0, 0x90, 60, 100];
        for pitch in 61..=66 {
            track.extend([0, pitch, 100]);
        }
        track.extend([1, 0xff, 0x2f, 0]);
        let bytes = fixture(0, 96, &[&track]);
        let import = decode(&bytes, PlaybackRate::OneX, Scope::Pattern).unwrap();
        assert_eq!(import.dropped_notes, 1);
        for (lane, pitch) in (60..66).enumerate() {
            assert_eq!(import.patterns[0].lanes[lane][0].midi, pitch);
            assert!(import.patterns[0].lanes[lane][0].active);
        }
    }

    #[test]
    fn malformed_or_oversized_files_fail_before_projection() {
        let valid = fixture(0, 96, &[&[0, 0xff, 0x2f, 0]]);
        for length in 0..valid.len() {
            assert!(decode(&valid[..length], PlaybackRate::OneX, Scope::Pattern).is_err());
        }
        for invalid in [
            fixture(2, 96, &[&[0, 0xff, 0x2f, 0]]),
            fixture(0, 0xe728, &[&[0, 0xff, 0x2f, 0]]),
            fixture(0, 96, &[&[0x80, 0x80, 0x80, 0x80, 0, 0xff, 0x2f, 0]]),
            fixture(0, 96, &[&[0, 60, 64, 0, 0xff, 0x2f, 0]]),
            fixture(0, 96, &[&[0, 0xff, 0x51, 3, 0, 0, 0, 0, 0xff, 0x2f, 0]]),
            fixture(0, 96, &[&[0, 0xff, 0x2f, 1, 0]]),
            vec![0; MAX_FILE_BYTES + 1],
        ] {
            assert!(decode(&invalid, PlaybackRate::OneX, Scope::Pattern).is_err());
        }
    }
#[test]
fn realtime_status_bytes_are_ignored_without_clearing_running_status() {
    let track = [
        0, 0x90, 60, 100,
        12, 0xf8,
        12, 60, 0,
        0, 0xfe,
        0, 64, 90,
        12, 0xfc,
        12, 64, 0,
        0, 0xff, 0x2f, 0,
        ];
    let bytes = fixture(0, 96, &[&track]);
    let import = decode(&bytes, PlaybackRate::OneX, Scope::Pattern)
    .expect("realtime bytes should be skipped");
    let pattern = &import.patterns[0];
    assert!(pattern.lanes[0][0].active);
    assert_eq!(pattern.lanes[0][0].midi, 60);
    assert_eq!(pattern.lanes[0][0].velocity, 100);
    assert!(pattern.lanes[0][1].active);
    assert_eq!(pattern.lanes[0][1].midi, 64);
    assert_eq!(pattern.lanes[0][1].velocity, 90);
}
}

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Instant;

use eframe::egui;
use leviathan_core::{
    CvFrame, SoundLane, SoundLinkPacket, NUM_LANES, SOUND_LINK_MAGIC, SOUND_LINK_VERSION,
    SOUND_PARAMS,
};

const PACKET_BYTES: usize = core::mem::size_of::<SoundLinkPacket>();
const DEFAULT_PORT: u16 = 9477;

const _: () = assert!(PACKET_BYTES == 336);

fn build_packet(cv: &CvFrame, playing: bool, sequence: u64, timestamp_ns: u64) -> SoundLinkPacket {
    let mut lanes = [SoundLane::default(); NUM_LANES];
    for (i, lane) in lanes.iter_mut().enumerate() {
        *lane = SoundLane {
            sound: cv.locks[i],
            gate: cv.gate_v[i],
            velocity: cv.velocity_v[i],
            pitch: cv.pitch_v[i],
        };
    }
    SoundLinkPacket {
        magic: SOUND_LINK_MAGIC,
        version: SOUND_LINK_VERSION,
        bytes: PACKET_BYTES as u32,
        sequence,
        timestamp_ns,
        playing: u8::from(playing),
        pad: [0; 7],
        beat: cv.beat,
        lanes,
    }
}

fn encode_packet(pkt: &SoundLinkPacket) -> [u8; PACKET_BYTES] {
    let mut buf = [0u8; PACKET_BYTES];
    buf[0..8].copy_from_slice(&pkt.magic.to_le_bytes());
    buf[8..12].copy_from_slice(&pkt.version.to_le_bytes());
    buf[12..16].copy_from_slice(&pkt.bytes.to_le_bytes());
    buf[16..24].copy_from_slice(&pkt.sequence.to_le_bytes());
    buf[24..32].copy_from_slice(&pkt.timestamp_ns.to_le_bytes());
    buf[32] = pkt.playing;
    buf[33..40].copy_from_slice(&pkt.pad);
    buf[40..48].copy_from_slice(&pkt.beat.to_le_bytes());

    for (idx, lane) in pkt.lanes.iter().enumerate() {
        let base = 48 + idx * 48;
        buf[base..base + 2].copy_from_slice(&lane.sound.machine.to_le_bytes());
        buf[base + 2..base + 4].copy_from_slice(&lane.sound.mask.to_le_bytes());
        for slot in 0..SOUND_PARAMS {
            let off = base + 4 + slot * 4;
            buf[off..off + 4].copy_from_slice(&lane.sound.values[slot].to_le_bytes());
        }
        buf[base + 36..base + 40].copy_from_slice(&lane.gate.to_le_bytes());
        buf[base + 40..base + 44].copy_from_slice(&lane.velocity.to_le_bytes());
        buf[base + 44..base + 48].copy_from_slice(&lane.pitch.to_le_bytes());
    }
    buf
}

#[cfg(test)]
fn decode_packet(buf: &[u8]) -> Result<SoundLinkPacket, &'static str> {
    if buf.len() != PACKET_BYTES {
        return Err("Invalid SoundLinkPacket byte length");
    }
    let magic = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    if magic != SOUND_LINK_MAGIC {
        return Err("Invalid SoundLinkPacket magic header");
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != SOUND_LINK_VERSION {
        return Err("Unsupported SoundLinkPacket version");
    }
    let bytes = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    if bytes != PACKET_BYTES as u32 {
        return Err("Invalid SoundLinkPacket size header");
    }
    let sequence = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let timestamp_ns = u64::from_le_bytes(buf[24..32].try_into().unwrap());
    let playing = buf[32];
    if playing > 1 {
        return Err("Invalid SoundLinkPacket playing flag");
    }
    let mut pad = [0u8; 7];
    pad.copy_from_slice(&buf[33..40]);
    if pad != [0u8; 7] {
        return Err("Non-zero SoundLinkPacket header padding");
    }
    let beat = f64::from_le_bytes(buf[40..48].try_into().unwrap());

    let mut lanes = [SoundLane::default(); NUM_LANES];
    for (idx, lane) in lanes.iter_mut().enumerate() {
        let base = 48 + idx * 48;
        let machine = i16::from_le_bytes(buf[base..base + 2].try_into().unwrap());
        let mask = u16::from_le_bytes(buf[base + 2..base + 4].try_into().unwrap());
        let mut values = [0.0f32; SOUND_PARAMS];
        for (slot, val) in values.iter_mut().enumerate() {
            let off = base + 4 + slot * 4;
            *val = f32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        }
        let gate = f32::from_le_bytes(buf[base + 36..base + 40].try_into().unwrap());
        let velocity = f32::from_le_bytes(buf[base + 40..base + 44].try_into().unwrap());
        let pitch = f32::from_le_bytes(buf[base + 44..base + 48].try_into().unwrap());
        *lane = SoundLane {
            sound: leviathan_core::SoundLock {
                machine,
                mask,
                values,
            },
            gate,
            velocity,
            pitch,
        };
    }

    Ok(SoundLinkPacket {
        magic,
        version,
        bytes,
        sequence,
        timestamp_ns,
        playing,
        pad,
        beat,
        lanes,
    })
}

pub struct SoundLinkBridge {
    enabled: bool,
    port: u16,
    socket: Option<UdpSocket>,
    sequence: u64,
    tx_packets: u64,
    start_instant: Instant,
    last_error: Option<String>,
}

impl Default for SoundLinkBridge {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            socket: None,
            sequence: 0,
            tx_packets: 0,
            start_instant: Instant::now(),
            last_error: None,
        }
    }
}

impl SoundLinkBridge {
    fn set_enabled(&mut self, enable: bool) -> Result<(), String> {
        if !enable {
            self.enabled = false;
            self.socket = None;
            self.last_error = None;
            return Ok(());
        }
        if self.socket.is_none() {
            let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
            let sock = UdpSocket::bind(bind_addr).map_err(|e| format!("UDP bind failed: {e}"))?;
            sock.set_nonblocking(true)
                .map_err(|e| format!("UDP nonblocking failed: {e}"))?;
            self.socket = Some(sock);
        }
        self.enabled = true;
        self.last_error = None;
        Ok(())
    }

    #[cfg(test)]
    fn set_port(&mut self, port: u16) {
        self.port = port.max(1);
    }

    pub fn emit_frame(&mut self, cv: &CvFrame, playing: bool) {
        if !self.enabled {
            return;
        }
        let Some(sock) = self.socket.as_ref() else {
            return;
        };
        self.sequence = self.sequence.wrapping_add(1);
        let timestamp_ns = self.start_instant.elapsed().as_nanos() as u64;
        let pkt = build_packet(cv, playing, self.sequence, timestamp_ns);
        let wire = encode_packet(&pkt);
        let target = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port));
        match sock.send_to(&wire, target) {
            Ok(sent) if sent == PACKET_BYTES => {
                self.tx_packets = self.tx_packets.wrapping_add(1);
                self.last_error = None;
            }
            Ok(sent) => {
                self.last_error = Some(format!("Short UDP datagram ({sent}/{PACKET_BYTES} B)"));
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(err) => {
                self.last_error = Some(format!("UDP send error: {err}"));
            }
        }
    }

    pub fn draw_controls(&mut self, ui: &mut egui::Ui) {
        let hover = match &self.last_error {
            Some(err) => format!("SoundLink UDP error: {err}"),
            None => format!(
                "Broadcast 336-byte SoundLinkPacket over loopback UDP (sent: {})",
                self.tx_packets
            ),
        };
        if ui
            .selectable_label(self.enabled, "LINK")
            .on_hover_text(hover)
            .clicked()
        {
            let target_state = !self.enabled;
            if let Err(err) = self.set_enabled(target_state) {
                self.last_error = Some(err);
            }
        }
        let mut port_val = self.port;
        if ui
            .add(egui::DragValue::new(&mut port_val).range(1024..=65535))
            .on_hover_text("Loopback UDP destination port")
            .changed()
        {
            self.port = port_val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sound_link_packet_round_trips_336_bytes_and_rejects_bad_headers() {
        let mut cv = CvFrame::default();
        cv.beat = 12.75;
        cv.gate_v[0] = 10.0;
        cv.velocity_v[0] = 8.5;
        cv.pitch_v[0] = -1.0;
        cv.locks[0] = leviathan_core::SoundLock {
            machine: 4,
            mask: 0b0101,
            values: [0.25, 0.0, 0.75, 0.0, 0.5, 0.0, 0.0, 1.0],
        };

        let pkt = build_packet(&cv, true, 77, 123_456_789);
        let mut wire = encode_packet(&pkt);
        assert_eq!(wire.len(), 336);
        let decoded = decode_packet(&wire).unwrap();
        assert_eq!(decoded, pkt);

        assert!(decode_packet(&wire[..335]).is_err());

        wire[8] = 99;
        assert!(decode_packet(&wire).unwrap_err().contains("version"));

        wire[8..12].copy_from_slice(&SOUND_LINK_VERSION.to_le_bytes());
        wire[0] ^= 0xFF;
        assert!(decode_packet(&wire).unwrap_err().contains("magic"));
    }

    #[test]
    fn non_blocking_loopback_udp_bridge_transmits_valid_336_byte_datagram() {
        let bind_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let rx_sock = UdpSocket::bind(bind_addr).unwrap();
        rx_sock
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let rx_port = rx_sock.local_addr().unwrap().port();

        let mut bridge = SoundLinkBridge::default();
        bridge.set_port(rx_port);
        bridge.set_enabled(true).unwrap();

        let mut cv = CvFrame::default();
        cv.beat = 3.5;
        cv.gate_v[1] = 10.0;
        cv.pitch_v[1] = 0.5;
        cv.locks[1].machine = 1;
        cv.locks[1].mask = 0x03;
        cv.locks[1].values[0] = 0.42;

        bridge.emit_frame(&cv, true);
        assert_eq!(bridge.tx_packets, 1);

        let mut recv_buf = [0u8; 512];
        let (len, _) = rx_sock.recv_from(&mut recv_buf).unwrap();
        assert_eq!(len, 336);

        let pkt = decode_packet(&recv_buf[..len]).unwrap();
        assert_eq!(pkt.magic, SOUND_LINK_MAGIC);
        assert_eq!(pkt.version, SOUND_LINK_VERSION);
        assert_eq!(pkt.bytes, 336);
        assert_eq!(pkt.sequence, 1);
        assert_eq!(pkt.playing, 1);
        assert!((pkt.beat - 3.5).abs() < 1e-12);
        assert_eq!(pkt.lanes[1].sound.machine, 1);
        assert_eq!(pkt.lanes[1].sound.mask, 0x03);
        assert!((pkt.lanes[1].sound.values[0] - 0.42).abs() < 1e-6);
    }

    #[test]
    fn link_controls_render_headlessly_without_error() {
        let ctx = egui::Context::default();
        let mut bridge = SoundLinkBridge::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                bridge.draw_controls(ui);
            });
        });
        assert!(!bridge.enabled);
    }
}

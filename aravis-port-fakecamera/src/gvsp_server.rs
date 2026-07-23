use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aravis_port_core::bootstrap::offset;
use aravis_port_core::gvsp::{ContentType, GvspHeader, GvspStatus, ImageInfos, LeaderPayload, PayloadKind, TrailerPayload};

use crate::gvcp_server::SharedState;
use crate::loss::LossInjector;
use crate::pattern;
use crate::registers::feature;

const HEADER_OVERHEAD: usize = 8; // 2-byte status + 6-byte standard GVSP header
const HAS_CHUNKS_BIT: u16 = 0x4000;

/// Chunk id used for the single "FrameID" chunk this fake camera appends when chunk mode is
/// active — a 4-byte big-endian copy of the frame id, matching the reverse-TLV layout
/// `aravis_port_memory::ChunkTlvIndex` expects.
pub const CHUNK_ID_FRAME_ID: u32 = 1;

pub(crate) fn spawn(
    shared: Arc<Mutex<SharedState>>,
    frame_period: Duration,
    packet_size: u16,
    loss_probability: f64,
) -> std::io::Result<(mpsc::Sender<()>, JoinHandle<()>)> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let (stop_tx, stop_rx) = mpsc::channel();
    let join = thread::spawn(move || run(socket, shared, frame_period, packet_size, loss_probability, stop_rx));
    Ok((stop_tx, join))
}

fn run(
    socket: UdpSocket,
    shared: Arc<Mutex<SharedState>>,
    frame_period: Duration,
    packet_size: u16,
    loss_probability: f64,
    stop_rx: mpsc::Receiver<()>,
) {
    let mut frame_id: u64 = 1;
    let loss = LossInjector::new(loss_probability, 0x9e37_79b9_7f4a_7c15 ^ frame_period.as_nanos() as u64);
    loop {
        match stop_rx.recv_timeout(frame_period) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let (dest, width, height, pixel_format, active, chunk_mode) = {
            let state = shared.lock().unwrap();
            let dest_ip = Ipv4Addr::from(state.bank.read_u32(offset::STREAM_CHANNEL_0_IP));
            // Port occupies the low 16 bits — matches `Device::open_stream_channel` and the
            // empirically-confirmed behavior of the live C5-2040-GigE camera.
            let port = (state.bank.read_u32(offset::STREAM_CHANNEL_0_PORT) & 0xffff) as u16;
            (
                SocketAddrV4::new(dest_ip, port),
                state.bank.read_u32(feature::WIDTH),
                state.bank.read_u32(feature::HEIGHT),
                state.bank.read_u32(feature::PIXEL_FORMAT),
                state.bank.read_u32(feature::ACQUISITION_ACTIVE) != 0,
                state.bank.read_u32(feature::CHUNK_MODE_ACTIVE) != 0,
            )
        };

        if !active || dest.ip().is_unspecified() || dest.port() == 0 {
            continue;
        }

        send_frame(&socket, dest, frame_id, width, height, pixel_format, packet_size, chunk_mode, &loss);
        frame_id = frame_id.wrapping_add(1).max(1);
    }
}

fn now_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

fn send_gvsp(socket: &UdpSocket, dest: SocketAddrV4, frame_id: u64, content_type: ContentType, packet_id: u32, payload: &[u8]) {
    let header = GvspHeader::Standard {
        frame_id: frame_id as u16,
        content_type,
        packet_id,
    };
    let mut packet = header.to_bytes(GvspStatus::Success);
    packet.extend_from_slice(payload);
    let _ = socket.send_to(&packet, dest);
}

/// Builds every packet (content type, packet id, payload bytes) that make up a frame. Pure
/// function of its arguments, so a resend request can regenerate any subset of a frame's packets
/// on demand without the server needing to retain per-frame history. When `chunk_mode` is set, a
/// single "FrameID" chunk (see [`CHUNK_ID_FRAME_ID`]) is appended after the image data, in the
/// reverse-TLV layout `aravis_port_memory::ChunkTlvIndex` expects, and the leader's
/// `has_chunks` bit is set.
fn build_frame_packets(
    frame_id: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
    packet_size: u16,
    chunk_mode: bool,
) -> Vec<(ContentType, u32, Vec<u8>)> {
    let mut image = pattern::generate_mono8(width, height, frame_id);
    let mut payload_type = PayloadKind::Image.to_u16();
    if chunk_mode {
        payload_type |= HAS_CHUNKS_BIT;
        let chunk_data = (frame_id as u32).to_be_bytes();
        image.extend_from_slice(&chunk_data);
        image.extend_from_slice(&CHUNK_ID_FRAME_ID.to_be_bytes());
        image.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
    }
    let capacity = (packet_size as usize).saturating_sub(HEADER_OVERHEAD).max(1);
    let mut packets = Vec::new();

    let leader = LeaderPayload {
        flags: 0,
        payload_type,
        timestamp: now_ns(),
        image: Some(ImageInfos {
            pixel_format,
            width,
            height,
            x_offset: 0,
            y_offset: 0,
            x_padding: 0,
            y_padding: 0,
        }),
    };
    packets.push((ContentType::Leader, 0u32, leader.encode()));

    let mut packet_id = 1u32;
    for chunk in image.chunks(capacity) {
        packets.push((ContentType::Payload, packet_id, chunk.to_vec()));
        packet_id += 1;
    }

    let trailer = TrailerPayload {
        payload_type: 1,
        data0: height,
    };
    packets.push((ContentType::Trailer, packet_id, trailer.encode()));

    packets
}

#[allow(clippy::too_many_arguments)]
fn send_frame(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    frame_id: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
    packet_size: u16,
    chunk_mode: bool,
    loss: &LossInjector,
) {
    for (content_type, packet_id, payload) in build_frame_packets(frame_id, width, height, pixel_format, packet_size, chunk_mode) {
        if loss.should_drop() {
            continue;
        }
        send_gvsp(socket, dest, frame_id, content_type, packet_id, &payload);
    }
}

/// Re-send packets `first..=last` of a previously-sent frame, reconstructing them from scratch
/// (see [`build_frame_packets`]) rather than replaying cached bytes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resend_packets(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    frame_id: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
    packet_size: u16,
    chunk_mode: bool,
    first: u32,
    last: u32,
) {
    for (content_type, packet_id, payload) in build_frame_packets(frame_id, width, height, pixel_format, packet_size, chunk_mode) {
        if packet_id >= first && packet_id <= last {
            send_gvsp(socket, dest, frame_id, content_type, packet_id, &payload);
        }
    }
}

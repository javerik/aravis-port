use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aravis_port_core::bootstrap::offset;
use aravis_port_core::gvsp::{
    ContentType, GvspHeader, GvspStatus, ImageInfos, LeaderPayload, PayloadKind, TrailerPayload,
};

use crate::gvcp_server::SharedState;
use crate::loss::LossInjector;
use crate::pattern;
use crate::registers::feature;

/// `GevSCPSPacketSize` budgets the entire wire datagram, so a real camera fits its GVSP payload
/// into what's left after the IP and UDP headers as well: 20 (IP) + 8 (UDP) + 2 (status) +
/// 6 (standard GVSP header). Modelling only the 8 GVSP bytes here would make this fake agree
/// with a reassembler that made the same mistake, hiding a stride bug that real hardware
/// exposes immediately.
const HEADER_OVERHEAD: usize = 20 + 8 + 2 + 6;
const HAS_CHUNKS_BIT: u16 = 0x4000;

/// Chunk id used for the single "FrameID" chunk this fake camera appends when chunk mode is
/// active — a 4-byte big-endian copy of the frame id, matching the reverse-TLV layout
/// `aravis_port_core::memory::ChunkTlvIndex` expects.
pub const CHUNK_ID_FRAME_ID: u32 = 1;

/// Chunk id of the image itself in chunk mode. GigE Vision chunk data wraps the image as the
/// first chunk, so the whole payload is a chain of `[data][id][size]` blocks; this id is the one
/// the live C6-2040-GigE uses.
pub const CHUNK_ID_IMAGE: u32 = 0xa6a6_a6a6;

/// Bytes chunk mode adds after the image: the image chunk's `[id][size]` trailer, then the
/// 4-byte FrameID chunk with its own trailer.
pub const CHUNK_MODE_EXTRA_BYTES: u32 = 8 + 4 + 8;

/// The packet size a real camera would actually packetize with: whatever the controller
/// negotiated via `GevSCPSPacketSize`, falling back to this fake's configured default when the
/// register was never written. The live-send and resend paths must agree on this — if one used
/// the negotiated value and the other the configured default, resent packets would carry a
/// different payload length and land at the wrong offsets.
pub(crate) fn negotiated_packet_size(bank: &crate::registers::RegisterBank, fallback: u16) -> u16 {
    let negotiated = (bank.read_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE) & 0xffff) as u16;
    if negotiated > 0 {
        negotiated
    } else {
        fallback
    }
}

pub(crate) fn spawn(
    shared: Arc<Mutex<SharedState>>,
    frame_period: Duration,
    packet_size: u16,
    loss_probability: f64,
) -> std::io::Result<(mpsc::Sender<()>, JoinHandle<()>)> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let (stop_tx, stop_rx) = mpsc::channel();
    let join = thread::spawn(move || {
        run(
            socket,
            shared,
            frame_period,
            packet_size,
            loss_probability,
            stop_rx,
        )
    });
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
    let loss = LossInjector::new(
        loss_probability,
        0x9e37_79b9_7f4a_7c15 ^ frame_period.as_nanos() as u64,
    );
    loop {
        match stop_rx.recv_timeout(frame_period) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let (dest, width, height, pixel_format, active, chunk_mode, packet_size, path_mtu) = {
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
                negotiated_packet_size(&state.bank, packet_size),
                state.path_mtu,
            )
        };

        // Packets bigger than the path MTU don't arrive, as on a real link.
        if !active || dest.ip().is_unspecified() || dest.port() == 0 || packet_size > path_mtu {
            continue;
        }

        send_frame(
            &socket,
            dest,
            frame_id,
            width,
            height,
            pixel_format,
            packet_size,
            chunk_mode,
            &loss,
        );
        frame_id = frame_id.wrapping_add(1).max(1);
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn send_gvsp(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    frame_id: u64,
    content_type: ContentType,
    packet_id: u32,
    payload: &[u8],
) {
    send_gvsp_with_status(
        socket,
        dest,
        GvspStatus::Success,
        frame_id,
        content_type,
        packet_id,
        payload,
    );
}

fn send_gvsp_with_status(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    status: GvspStatus,
    frame_id: u64,
    content_type: ContentType,
    packet_id: u32,
    payload: &[u8],
) {
    let header = GvspHeader::Standard {
        frame_id: frame_id as u16,
        content_type,
        packet_id,
    };
    let mut packet = header.to_bytes(status);
    packet.extend_from_slice(payload);
    let _ = socket.send_to(&packet, dest);
}

/// GEV_STATUS_PACKET_UNAVAILABLE: the requested packet is no longer in the device's send buffer.
const STATUS_PACKET_UNAVAILABLE: u16 = 0x800c;

/// Answer a resend of packets `first..=last` with one data-less "packet unavailable" error
/// packet each, as a device does once it has dropped them.
pub(crate) fn send_unavailable(
    socket: &UdpSocket,
    dest: SocketAddrV4,
    frame_id: u64,
    first: u32,
    last: u32,
) {
    for packet_id in first..=last {
        send_gvsp_with_status(
            socket,
            dest,
            GvspStatus::Error(STATUS_PACKET_UNAVAILABLE),
            frame_id,
            ContentType::Payload,
            packet_id,
            &[],
        );
    }
}

/// Builds every packet (content type, packet id, payload bytes) that make up a frame. Pure
/// function of its arguments, so a resend request can regenerate any subset of a frame's packets
/// on demand without the server needing to retain per-frame history. When `chunk_mode` is set,
/// the image is wrapped as a chunk (see [`CHUNK_ID_IMAGE`]) followed by a "FrameID" chunk (see
/// [`CHUNK_ID_FRAME_ID`]), in the reverse-TLV layout `aravis_port_core::memory::ChunkTlvIndex`
/// expects, and the leader's `has_chunks` bit is set.
fn build_frame_packets(
    frame_id: u64,
    width: u32,
    height: u32,
    pixel_format: u32,
    packet_size: u16,
    chunk_mode: bool,
) -> Vec<(ContentType, u32, Vec<u8>)> {
    let mut image = pattern::generate(width, height, frame_id, pixel_format);
    let mut payload_type = PayloadKind::Image.to_u16();
    if chunk_mode {
        payload_type |= HAS_CHUNKS_BIT;
        let image_len = image.len() as u32;
        image.extend_from_slice(&CHUNK_ID_IMAGE.to_be_bytes());
        image.extend_from_slice(&image_len.to_be_bytes());
        let chunk_data = (frame_id as u32).to_be_bytes();
        image.extend_from_slice(&chunk_data);
        image.extend_from_slice(&CHUNK_ID_FRAME_ID.to_be_bytes());
        image.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
    }
    let capacity = (packet_size as usize)
        .saturating_sub(HEADER_OVERHEAD)
        .max(1);
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
    for (content_type, packet_id, payload) in build_frame_packets(
        frame_id,
        width,
        height,
        pixel_format,
        packet_size,
        chunk_mode,
    ) {
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
    for (content_type, packet_id, payload) in build_frame_packets(
        frame_id,
        width,
        height,
        pixel_format,
        packet_size,
        chunk_mode,
    ) {
        if packet_id >= first && packet_id <= last {
            send_gvsp(socket, dest, frame_id, content_type, packet_id, &payload);
        }
    }
}

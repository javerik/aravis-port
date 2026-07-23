use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aravis_port_core::bootstrap::offset;
use aravis_port_core::gvcp::{
    Command, GvcpHeader, GvcpPayload, PacketFlags, PacketResend, PacketType, ReadMemoryAck,
    ReadMemoryCmd, ReadRegisterAck, ReadRegisterCmd, WriteMemoryAck, WriteMemoryCmd,
    WriteRegisterAck, WriteRegisterCmd, HEADER_LEN,
};

use crate::registers::{feature, RegisterBank};

/// GVCP error code returned for writes from a non-controller client. Our own choice — not
/// necessarily the official GEV error code, since this is a simulator, not a spec-compliance
/// reference.
const ERROR_WRITE_ACCESS_DENIED: u8 = 0x06;

pub(crate) struct SharedState {
    pub bank: RegisterBank,
    pub controller: Option<SocketAddr>,
    pub heartbeat_deadline: Instant,
    pub packet_size: u16,
}

pub(crate) fn spawn(
    socket: UdpSocket,
    shared: Arc<Mutex<SharedState>>,
    heartbeat_timeout: Duration,
) -> (mpsc::Sender<()>, JoinHandle<()>) {
    let (stop_tx, stop_rx) = mpsc::channel();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set_read_timeout on a freshly bound socket cannot fail");
    let join = thread::spawn(move || run(socket, shared, stop_rx, heartbeat_timeout));
    (stop_tx, join)
}

fn run(socket: UdpSocket, shared: Arc<Mutex<SharedState>>, stop_rx: mpsc::Receiver<()>, heartbeat_timeout: Duration) {
    let mut buf = [0u8; 1024];
    loop {
        if stop_rx.try_recv().is_ok() {
            return;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => handle_packet(&socket, &shared, &buf[..n], from, heartbeat_timeout),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(_) => continue,
        }
    }
}

fn can_write(state: &SharedState, from: SocketAddr) -> bool {
    match state.controller {
        None => true,
        Some(c) => c == from,
    }
}

fn touch_heartbeat_if_controller(state: &mut SharedState, from: SocketAddr, heartbeat_timeout: Duration) {
    if state.controller == Some(from) {
        state.heartbeat_deadline = Instant::now() + heartbeat_timeout;
    }
}

fn expire_stale_controller(state: &mut SharedState, from: SocketAddr) {
    if let Some(controller) = state.controller {
        if controller != from && Instant::now() > state.heartbeat_deadline {
            state.controller = None;
        }
    }
}

fn handle_packet(
    socket: &UdpSocket,
    shared: &Arc<Mutex<SharedState>>,
    bytes: &[u8],
    from: SocketAddr,
    heartbeat_timeout: Duration,
) {
    let Ok(header) = GvcpHeader::from_bytes(bytes) else {
        return;
    };
    let body = if bytes.len() > HEADER_LEN { &bytes[HEADER_LEN..] } else { &[] };
    let mut state = shared.lock().unwrap();
    expire_stale_controller(&mut state, from);

    match header.command {
        Command::DiscoveryCmd => {
            let ack_bytes = state.bank.discovery_ack_bytes();
            send_ack(socket, from, Command::DiscoveryAck, header.id, &ack_bytes);
        }
        Command::ReadRegisterCmd => {
            let Ok(cmd) = ReadRegisterCmd::decode(body) else { return };
            let value = state.bank.read_u32(cmd.address);
            touch_heartbeat_if_controller(&mut state, from, heartbeat_timeout);
            send_ack(socket, from, Command::ReadRegisterAck, header.id, &ReadRegisterAck { value }.encode());
        }
        Command::WriteRegisterCmd => {
            let Ok(cmd) = WriteRegisterCmd::decode(body) else { return };
            if !can_write(&state, from) {
                send_error(socket, from, &header, ERROR_WRITE_ACCESS_DENIED);
                return;
            }
            if cmd.address == offset::CONTROL_CHANNEL_PRIVILEGE {
                if RegisterBank::has_control_flags(cmd.value) {
                    state.controller = Some(from);
                    state.heartbeat_deadline = Instant::now() + heartbeat_timeout;
                } else if state.controller == Some(from) {
                    state.controller = None;
                }
            }
            state.bank.write_u32(cmd.address, cmd.value);
            touch_heartbeat_if_controller(&mut state, from, heartbeat_timeout);
            send_ack(
                socket,
                from,
                Command::WriteRegisterAck,
                header.id,
                &WriteRegisterAck { data_index: 0 }.encode(),
            );
        }
        Command::ReadMemoryCmd => {
            let Ok(cmd) = ReadMemoryCmd::decode(body) else { return };
            let data = state.bank.read(cmd.address, cmd.size as usize);
            touch_heartbeat_if_controller(&mut state, from, heartbeat_timeout);
            send_ack(
                socket,
                from,
                Command::ReadMemoryAck,
                header.id,
                &ReadMemoryAck { address: cmd.address, data }.encode(),
            );
        }
        Command::WriteMemoryCmd => {
            let Ok(cmd) = WriteMemoryCmd::decode(body) else { return };
            if !can_write(&state, from) {
                send_error(socket, from, &header, ERROR_WRITE_ACCESS_DENIED);
                return;
            }
            state.bank.write(cmd.address, &cmd.data);
            touch_heartbeat_if_controller(&mut state, from, heartbeat_timeout);
            send_ack(
                socket,
                from,
                Command::WriteMemoryAck,
                header.id,
                &WriteMemoryAck { address: cmd.address }.encode(),
            );
        }
        Command::PacketResendCmd => {
            // No ack is sent for a resend request — the device just re-emits the requested GVSP
            // packets on the stream socket, matching real GVCP behavior.
            let extended = header.raw_flags & PacketFlags::EXTENDED_IDS.bits() != 0;
            handle_resend(&state, body, extended);
        }
        _ => {}
    }
}

fn handle_resend(state: &SharedState, body: &[u8], extended: bool) {
    let Ok(resend) = PacketResend::decode(body, extended) else { return };
    let (frame_id, first, last) = match resend {
        PacketResend::Standard {
            frame_id,
            first_block,
            last_block,
        } => (frame_id as u64, first_block, last_block),
        PacketResend::Extended {
            frame_id,
            first_block,
            last_block,
        } => (frame_id, first_block, last_block),
    };

    let dest_ip = Ipv4Addr::from(state.bank.read_u32(offset::STREAM_CHANNEL_0_IP));
    let port = (state.bank.read_u32(offset::STREAM_CHANNEL_0_PORT) & 0xffff) as u16;
    let dest = SocketAddrV4::new(dest_ip, port);
    if dest.ip().is_unspecified() || dest.port() == 0 {
        return;
    }

    let width = state.bank.read_u32(feature::WIDTH);
    let height = state.bank.read_u32(feature::HEIGHT);
    let pixel_format = state.bank.read_u32(feature::PIXEL_FORMAT);
    let chunk_mode = state.bank.read_u32(feature::CHUNK_MODE_ACTIVE) != 0;

    // A fresh, ephemeral socket for the resend: UDP is connectionless and the receiver doesn't
    // validate the sender's source port, so there's no need to share the GVSP sender's socket
    // (and thus no cross-thread synchronization needed for this rare, low-volume path).
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return };
    crate::gvsp_server::resend_packets(
        &socket,
        dest,
        frame_id,
        width,
        height,
        pixel_format,
        state.packet_size,
        chunk_mode,
        first,
        last,
    );
}

fn send_ack(socket: &UdpSocket, to: SocketAddr, command: Command, id: u16, payload: &[u8]) {
    let header = GvcpHeader {
        packet_type: PacketType::Ack,
        raw_flags: 0,
        command,
        size: payload.len() as u16,
        id,
    };
    let mut packet = header.to_bytes().to_vec();
    packet.extend_from_slice(payload);
    let _ = socket.send_to(&packet, to);
}

fn send_error(socket: &UdpSocket, to: SocketAddr, req: &GvcpHeader, code: u8) {
    let header = GvcpHeader {
        packet_type: PacketType::Error,
        raw_flags: code,
        command: req.command,
        size: 0,
        id: req.id,
    };
    let _ = socket.send_to(&header.to_bytes(), to);
}

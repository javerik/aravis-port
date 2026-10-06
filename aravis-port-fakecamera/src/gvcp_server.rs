use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use aravis_port_core::bootstrap::{offset, stream_packet_size};
use aravis_port_core::gvcp::{
    Command, GvcpHeader, GvcpPayload, PacketFlags, PacketResend, PacketType, ReadMemoryAck,
    ReadMemoryCmd, ReadRegisterAck, ReadRegisterCmd, WriteMemoryAck, WriteMemoryCmd,
    WriteRegisterAck, WriteRegisterCmd, HEADER_LEN,
};

use crate::registers::{default_pixel_format, feature, pixel_format_allowed, RegisterBank};

/// GVCP error code returned for writes from a non-controller client. Our own choice — not
/// necessarily the official GEV error code, since this is a simulator, not a spec-compliance
/// reference.
const ERROR_WRITE_ACCESS_DENIED: u8 = 0x06;
/// GVCP error code for a value the device won't take in its current state: a `PixelFormat` the
/// current `DeviceScanType` doesn't stream. Numbered like Aravis's `ArvGvcpError`.
const ERROR_INVALID_PARAMETER: u8 = 0x02;

pub(crate) struct SharedState {
    pub bank: RegisterBank,
    pub controller: Option<SocketAddr>,
    pub heartbeat_deadline: Instant,
    pub packet_size: u16,
    pub path_mtu: u16,
    pub test_packets: bool,
    pub resend_unavailable: bool,
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
            if refuses_write(&state.bank, cmd.address, &cmd.value.to_be_bytes()) {
                send_error(socket, from, &header, ERROR_INVALID_PARAMETER);
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
            after_write(&mut state, cmd.address, 4);
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
            if refuses_write(&state.bank, cmd.address, &cmd.data) {
                send_error(socket, from, &header, ERROR_INVALID_PARAMETER);
                return;
            }
            state.bank.write(cmd.address, &cmd.data);
            after_write(&mut state, cmd.address, cmd.data.len());
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

/// Side effects of a register write covering `[address, address + len)`: a set
/// `GevSCPSFireTestPacket` bit sends one test packet and clears itself, as on a real device.
/// Whether the device turns down writing `data` at `address`: a `PixelFormat` the scan mode
/// (the one in the same write, if it sets both) doesn't stream.
fn refuses_write(bank: &RegisterBank, address: u32, data: &[u8]) -> bool {
    let Some(pixel_format) = written_u32(address, data, feature::PIXEL_FORMAT) else {
        return false;
    };
    let scan = written_u32(address, data, feature::DEVICE_SCAN_TYPE)
        .unwrap_or_else(|| bank.read_u32(feature::DEVICE_SCAN_TYPE));
    !pixel_format_allowed(scan, pixel_format)
}

/// The big-endian `u32` that writing `data` at `address` puts into `register`, if the write
/// covers all four of its bytes.
fn written_u32(address: u32, data: &[u8], register: u32) -> Option<u32> {
    let start = register.checked_sub(address)? as usize;
    let bytes = data.get(start..start.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn covers(address: u32, len: usize, register: u32) -> bool {
    (address..address.saturating_add(len as u32)).contains(&register)
}

fn after_write(state: &mut SharedState, address: u32, len: usize) {
    // A new scan mode brings its own pixel formats; the device moves onto one by itself.
    if covers(address, len, feature::DEVICE_SCAN_TYPE) {
        let scan = state.bank.read_u32(feature::DEVICE_SCAN_TYPE);
        if !pixel_format_allowed(scan, state.bank.read_u32(feature::PIXEL_FORMAT)) {
            state.bank.write_u32(feature::PIXEL_FORMAT, default_pixel_format(scan));
        }
    }

    let register = offset::STREAM_CHANNEL_0_PACKET_SIZE;
    if !covers(address, len, register) {
        return;
    }
    let value = state.bank.read_u32(register);
    if value & stream_packet_size::FIRE_TEST_PACKET == 0 {
        return;
    }
    state.bank.write_u32(register, value & !stream_packet_size::FIRE_TEST_PACKET);

    let size = (value & stream_packet_size::SIZE_MASK) as u16;
    if !state.test_packets || size > state.path_mtu {
        return;
    }
    let dest_ip = Ipv4Addr::from(state.bank.read_u32(offset::STREAM_CHANNEL_0_IP));
    let port = (state.bank.read_u32(offset::STREAM_CHANNEL_0_PORT) & 0xffff) as u16;
    if dest_ip.is_unspecified() || port == 0 {
        return;
    }
    // The whole datagram is `size` bytes, so the UDP payload is what's left after the IP (20)
    // and UDP (8) headers. Real devices fill it from an LFSR; any bytes do here.
    let payload: Vec<u8> = (0..(size as usize).saturating_sub(20 + 8)).map(|i| i as u8).collect();
    if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        let _ = socket.send_to(&payload, SocketAddrV4::new(dest_ip, port));
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
    if state.resend_unavailable {
        crate::gvsp_server::send_unavailable(&socket, dest, frame_id, first, last);
        return;
    }
    crate::gvsp_server::resend_packets(
        &socket,
        dest,
        frame_id,
        width,
        height,
        pixel_format,
        crate::gvsp_server::negotiated_packet_size(&state.bank, state.packet_size),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registers::{pixel_format, scan_type, Identity};

    fn bank(scan: u32) -> RegisterBank {
        let identity = Identity {
            manufacturer: "aravis-port".into(),
            model: "FakeCamera".into(),
            version: "0.1".into(),
            serial: "0001".into(),
            mac: aravis_port_core::MacAddress::new([0, 0x11, 0x22, 0x33, 0x44, 0x55]),
            current_ip: Ipv4Addr::LOCALHOST,
        };
        let mut bank = RegisterBank::new(&identity, vec![]);
        bank.write_u32(feature::DEVICE_SCAN_TYPE, scan);
        bank
    }

    #[test]
    fn a_pixel_format_outside_the_scan_mode_is_refused() {
        let line = bank(scan_type::LINESCAN3D);
        assert!(refuses_write(&line, feature::PIXEL_FORMAT, &pixel_format::MONO16.to_be_bytes()));
        assert!(!refuses_write(&line, feature::PIXEL_FORMAT, &pixel_format::COORD3D_C16.to_be_bytes()));
        let area = bank(scan_type::AREASCAN);
        assert!(refuses_write(&area, feature::PIXEL_FORMAT, &pixel_format::COORD3D_C16.to_be_bytes()));
        assert!(!refuses_write(&area, feature::PIXEL_FORMAT, &pixel_format::MONO8.to_be_bytes()));
    }

    #[test]
    fn other_registers_and_partial_writes_are_not_judged() {
        let line = bank(scan_type::LINESCAN3D);
        assert!(!refuses_write(&line, feature::WIDTH, &7u32.to_be_bytes()));
        assert!(!refuses_write(&line, feature::PIXEL_FORMAT + 2, &[0, 1]));
    }

    #[test]
    fn a_write_covering_both_registers_is_judged_by_its_own_scan_type() {
        // DEVICE_SCAN_TYPE lies past PIXEL_FORMAT; one memory write spanning both.
        let start = feature::PIXEL_FORMAT;
        let mut data = vec![0u8; (feature::DEVICE_SCAN_TYPE + 4 - start) as usize];
        data[..4].copy_from_slice(&pixel_format::COORD3D_C16.to_be_bytes());
        let at = (feature::DEVICE_SCAN_TYPE - start) as usize;
        data[at..at + 4].copy_from_slice(&scan_type::LINESCAN3D.to_be_bytes());
        assert!(!refuses_write(&bank(scan_type::AREASCAN), start, &data));
    }
}

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::thread;
use std::time::Duration;

use aravis_port_core::gvcp::{
    Command, GvcpHeader, GvcpPayload, PacketType, PendingAck, ReadRegisterAck,
};
use aravis_port_core::Error;
use aravis_port_device::net::{GvcpTransaction, TransactionConfig};

/// Binds a loopback "fake device" socket and returns it plus its address.
fn fake_device_socket() -> (UdpSocket, SocketAddrV4) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = match socket.local_addr().unwrap() {
        std::net::SocketAddr::V4(v4) => v4,
        _ => unreachable!(),
    };
    (socket, addr)
}

fn fast_config() -> TransactionConfig {
    TransactionConfig {
        retries: 2,
        timeout: Duration::from_millis(80),
    }
}

#[test]
fn never_responding_peer_times_out_after_retries() {
    let (_device_socket, device_addr) = fake_device_socket();
    let mut txn = GvcpTransaction::connect(device_addr, fast_config()).unwrap();

    let start = std::time::Instant::now();
    let result = txn.read_register(0xa00);
    assert!(matches!(result, Err(Error::Timeout { .. })));
    // Should have taken roughly (retries + 1) * timeout, not instant and not forever.
    assert!(start.elapsed() >= Duration::from_millis(80 * 3 - 10));
}

#[test]
fn pending_ack_extends_deadline_without_consuming_a_retry() {
    let (device_socket, device_addr) = fake_device_socket();
    let mut txn = GvcpTransaction::connect(
        device_addr,
        TransactionConfig {
            retries: 1,
            timeout: Duration::from_millis(50),
        },
    )
    .unwrap();

    let handle = thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let (n, from) = device_socket.recv_from(&mut buf).unwrap();
        let req = GvcpHeader::from_bytes(&buf[..n]).unwrap();
        assert_eq!(req.command, Command::ReadRegisterCmd);

        // Reply with a PENDING_ACK asking for much more time than the configured timeout.
        let pending = PendingAck { timeout_ms: 300 };
        let payload = pending.encode();
        let header = GvcpHeader {
            packet_type: PacketType::Ack,
            raw_flags: 0,
            command: Command::PendingAck,
            size: payload.len() as u16,
            id: req.id,
        };
        let mut packet = header.to_bytes().to_vec();
        packet.extend_from_slice(&payload);
        device_socket.send_to(&packet, from).unwrap();

        // Wait longer than the original 50ms timeout (but within the 300ms pending window)
        // before sending the real ack, proving the deadline was actually extended.
        thread::sleep(Duration::from_millis(150));

        let ack = ReadRegisterAck { value: 7 };
        let payload = ack.encode();
        let header = GvcpHeader {
            packet_type: PacketType::Ack,
            raw_flags: 0,
            command: Command::ReadRegisterAck,
            size: payload.len() as u16,
            id: req.id,
        };
        let mut packet = header.to_bytes().to_vec();
        packet.extend_from_slice(&payload);
        device_socket.send_to(&packet, from).unwrap();
    });

    let value = txn.read_register(0xa00).unwrap();
    assert_eq!(value, 7);
    handle.join().unwrap();
}

#[test]
fn mismatched_id_and_command_are_ignored_then_real_ack_accepted() {
    let (device_socket, device_addr) = fake_device_socket();
    let mut txn = GvcpTransaction::connect(device_addr, fast_config()).unwrap();

    let handle = thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let (n, from) = device_socket.recv_from(&mut buf).unwrap();
        let req = GvcpHeader::from_bytes(&buf[..n]).unwrap();

        // Wrong id.
        send_ack(
            &device_socket,
            from,
            Command::ReadRegisterAck,
            req.id.wrapping_add(1),
            &ReadRegisterAck { value: 1 }.encode(),
        );
        // Wrong command (matching id).
        send_ack(
            &device_socket,
            from,
            Command::WriteRegisterAck,
            req.id,
            &[0, 0, 0, 0],
        );
        // Correct ack.
        send_ack(
            &device_socket,
            from,
            Command::ReadRegisterAck,
            req.id,
            &ReadRegisterAck { value: 42 }.encode(),
        );
    });

    let value = txn.read_register(0xa00).unwrap();
    assert_eq!(value, 42);
    handle.join().unwrap();
}

#[test]
fn error_ack_is_returned_as_gvcp_error() {
    let (device_socket, device_addr) = fake_device_socket();
    let mut txn = GvcpTransaction::connect(device_addr, fast_config()).unwrap();

    let handle = thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let (n, from) = device_socket.recv_from(&mut buf).unwrap();
        let req = GvcpHeader::from_bytes(&buf[..n]).unwrap();
        let header = GvcpHeader {
            packet_type: PacketType::Error,
            raw_flags: 0x02, // ARV_GVCP_ERROR_INVALID_ADDRESS-ish
            command: req.command,
            size: 0,
            id: req.id,
        };
        device_socket.send_to(&header.to_bytes(), from).unwrap();
    });

    let result = txn.read_register(0xa00);
    assert!(matches!(result, Err(Error::GvcpError(0x02))));
    handle.join().unwrap();
}

fn send_ack(
    socket: &UdpSocket,
    to: std::net::SocketAddr,
    command: Command,
    id: u16,
    payload: &[u8],
) {
    let header = GvcpHeader {
        packet_type: PacketType::Ack,
        raw_flags: 0,
        command,
        size: payload.len() as u16,
        id,
    };
    let mut packet = header.to_bytes().to_vec();
    packet.extend_from_slice(payload);
    socket.send_to(&packet, to).unwrap();
}

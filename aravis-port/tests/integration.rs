//! The integration scenarios from `ai/rust-port.md`'s testing strategy, run against the
//! in-process fake camera: discovery, GVCP round-trip, streaming, resend-under-loss, chunk data,
//! plus a sixth scenario (heartbeat loss / control reacquisition) this plan added.

use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

use aravis_port::core::gvcp::{Command, DiscoveryAck, GvcpHeader, GvcpPayload, PacketType, HEADER_LEN};
use aravis_port::memory::{BufferStatus, ChunkTlvIndex};
use aravis_port::prelude::*;
use aravis_port::{Device, DeviceConfig};
use aravis_port_fakecamera::{feature, FakeCamera, FakeCameraConfig};

fn fake_camera(cfg: FakeCameraConfig) -> FakeCamera {
    FakeCamera::start(cfg).expect("failed to start fake camera")
}

/// Scenario 1 — Discovery. `aravis_port::discover()` itself is broadcast-only (validated against
/// real hardware in earlier phases) and targets the fixed GVCP port 3956, which the fake camera
/// deliberately doesn't bind (to avoid CI port conflicts) — so this exercises the same
/// `DISCOVERY_CMD`/`DISCOVERY_ACK` wire exchange via a direct unicast probe instead, which is
/// equally valid GVCP and proves the fake camera "appears" to a discovery request.
#[test]
fn discovery_finds_the_fake_camera() {
    let camera = fake_camera(FakeCameraConfig {
        model: "DiscoveryTestCamera".to_string(),
        ..Default::default()
    });

    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let header = GvcpHeader {
        packet_type: PacketType::Cmd,
        raw_flags: 0x01,
        command: Command::DiscoveryCmd,
        size: 0,
        id: 0xffff,
    };
    socket.send_to(&header.to_bytes(), camera.local_addr()).unwrap();

    socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut buf = [0u8; 1024];
    let (n, _from) = socket.recv_from(&mut buf).unwrap();
    let resp = GvcpHeader::from_bytes(&buf[..n]).unwrap();
    assert_eq!(resp.command, Command::DiscoveryAck);
    let ack = DiscoveryAck::decode(&buf[HEADER_LEN..n]).unwrap();
    assert_eq!(ack.model(), "DiscoveryTestCamera");
}

/// Scenario 2 — GVCP command round-trip: connect, write a register/feature, read it back.
#[test]
fn gvcp_write_then_read_round_trip() {
    let camera = fake_camera(FakeCameraConfig::default());
    let cam = Camera::connect_addr(camera.local_addr()).unwrap();

    cam.write::<f64>("ExposureTime", 42_000.0).unwrap();
    assert_eq!(cam.read::<f64>("ExposureTime").unwrap(), 42_000.0);
}

/// Scenario 3 — Streaming: start a stream through the umbrella `Camera` API, receive 10 frames.
#[test]
fn camera_start_stream_receives_ten_frames() {
    let camera = fake_camera(FakeCameraConfig {
        frame_period: Duration::from_millis(15),
        ..Default::default()
    });
    camera.poke_register(feature::WIDTH, 32);
    camera.poke_register(feature::HEIGHT, 32);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();

    let mut received = 0;
    for _ in 0..10 {
        let buf = rx.recv_timeout(Duration::from_secs(3)).expect("expected a frame");
        assert_eq!(buf.status, BufferStatus::Success);
        assert_eq!(buf.data().len(), 32 * 32);
        received += 1;
    }
    cam.stop_stream(stream).unwrap();
    assert_eq!(received, 10);
}

/// `start_stream_with_config` must actually apply the caller's packet size to the device
/// (`GevSCPSPacketSize`, not the hardcoded convenience default), and streaming must still work
/// end-to-end with it.
#[test]
fn camera_start_stream_with_config_uses_the_given_packet_size() {
    let camera = fake_camera(FakeCameraConfig {
        frame_period: Duration::from_millis(15),
        ..Default::default()
    });
    camera.poke_register(feature::WIDTH, 16);
    camera.poke_register(feature::HEIGHT, 16);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let cfg = StreamConfig {
        packet_size: 900,
        ..StreamConfig::default()
    };
    let stream = cam
        .start_stream_with_config(cfg, move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();

    let buf = rx.recv_timeout(Duration::from_secs(3)).expect("expected a frame");
    cam.stop_stream(stream).unwrap();

    assert_eq!(buf.status, BufferStatus::Success);
    assert_eq!(buf.data().len(), 16 * 16);
    let packet_size_reg = camera.peek_register(aravis_port::core::bootstrap::offset::STREAM_CHANNEL_0_PACKET_SIZE) & 0xffff;
    assert_eq!(packet_size_reg, 900, "device's GevSCPSPacketSize should reflect the custom config, not the 1400 default");
}

/// Scenario 4 — Resend under loss: covered thoroughly at the `aravis-port-stream` layer
/// (`tests/resend_under_loss.rs`, using a real GVCP-connected resend requester against the fake
/// camera's loss injector). Re-validated here through the umbrella `Camera` API specifically, to
/// confirm the convenience layer's own wiring (its `ResendRequester` impl, buffer pool sizing)
/// doesn't break resend behavior.
#[test]
fn camera_start_stream_recovers_frames_despite_packet_loss() {
    let camera = fake_camera(FakeCameraConfig {
        frame_period: Duration::from_millis(20),
        gvsp_loss_probability: 0.15,
        ..Default::default()
    });
    camera.poke_register(feature::WIDTH, 128);
    camera.poke_register(feature::HEIGHT, 128);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();

    let mut successes = 0;
    let mut total = 0;
    for _ in 0..15 {
        if let Ok(buf) = rx.recv_timeout(Duration::from_secs(3)) {
            total += 1;
            if buf.status == BufferStatus::Success {
                successes += 1;
            }
        }
    }
    cam.stop_stream(stream).unwrap();
    assert!(total >= 10);
    assert!(successes as f64 / total as f64 >= 0.7, "{successes}/{total} succeeded under 15% loss");
}

/// Scenario 5 — Chunk data: enable chunk mode, verify the appended chunk survives streaming and
/// `ChunkTlvIndex` can extract it, matching the actual frame id.
#[test]
fn chunk_data_round_trips_through_streaming() {
    let camera = fake_camera(FakeCameraConfig {
        frame_period: Duration::from_millis(15),
        ..Default::default()
    });
    camera.poke_register(feature::WIDTH, 16);
    camera.poke_register(feature::HEIGHT, 16);
    camera.poke_register(feature::CHUNK_MODE_ACTIVE, 1);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();

    let buf = rx.recv_timeout(Duration::from_secs(3)).expect("expected a frame");
    cam.stop_stream(stream).unwrap();

    assert_eq!(buf.status, BufferStatus::Success);
    let image_size = 16 * 16;
    let chunk_region = &buf.data()[image_size..];
    let index = ChunkTlvIndex::build(chunk_region).unwrap();
    let chunk_bytes = index
        .get(chunk_region, aravis_port_fakecamera::CHUNK_ID_FRAME_ID)
        .expect("expected a FrameID chunk");
    let chunk_frame_id = u32::from_be_bytes(chunk_bytes.try_into().unwrap());
    assert_eq!(chunk_frame_id as u64, buf.frame_id);
}

/// Scenario 6 — Heartbeat loss / control reacquisition: a device that stops heartbeating loses
/// control, and a second client can then acquire it.
#[test]
fn heartbeat_loss_allows_control_reacquisition() {
    let camera = fake_camera(FakeCameraConfig {
        heartbeat_timeout: Duration::from_millis(150),
        ..Default::default()
    });

    let mut first = Device::connect(
        camera.local_addr(),
        DeviceConfig {
            heartbeat_period: Duration::from_millis(1_000_000), // effectively never heartbeats again
            ..Default::default()
        },
    )
    .unwrap();
    assert!(first.has_control());
    first.stop_heartbeat();

    // Wait past the fake camera's heartbeat timeout so it expires the first client's privilege.
    std::thread::sleep(Duration::from_millis(400));

    let second = Device::connect(camera.local_addr(), DeviceConfig::default()).unwrap();
    assert!(second.has_control());
}

//! The fake camera as a networked device: binding GVCP/GVSP where the caller says, answering
//! discovery only within its subnet, frames from a caller-supplied `FrameSource` (swapped while
//! streaming, replayed for resends) and frame pacing from `AcquisitionFrameRate`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aravis_port::core::bootstrap::offset;
use aravis_port::core::gvcp::{
    Command, DiscoveryAck, GvcpHeader, GvcpPayload, PacketType, HEADER_LEN,
};
use aravis_port::core::gvsp::{ContentType, GvspHeader};
use aravis_port::memory::BufferStatus;
use aravis_port::prelude::*;
use aravis_port_fakecamera::{
    feature, pixel_format, FakeCamera, FakeCameraConfig, FrameRequest, PatternSource,
};

/// A second loopback address, so "the camera's IP" differs from the test's own `127.0.0.1`.
const CAMERA_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);

fn discovery_ack(from: Ipv4Addr, camera: SocketAddrV4) -> Option<DiscoveryAck> {
    let socket = UdpSocket::bind((from, 0)).unwrap();
    let header = GvcpHeader {
        packet_type: PacketType::Cmd,
        raw_flags: 0x01,
        command: Command::DiscoveryCmd,
        size: 0,
        id: 0x1234,
    };
    socket.send_to(&header.to_bytes(), camera).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut buf = [0u8; 1024];
    let (n, _) = socket.recv_from(&mut buf).ok()?;
    let resp = GvcpHeader::from_bytes(&buf[..n]).unwrap();
    assert_eq!(resp.command, Command::DiscoveryAck);
    Some(DiscoveryAck::decode(&buf[HEADER_LEN..n]).unwrap())
}

/// A plain UDP socket the camera streams to, set up by poking the stream channel registers.
fn stream_to_raw_socket(camera: &FakeCamera) -> UdpSocket {
    let receiver = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let port = receiver.local_addr().unwrap().port();
    camera.poke_register(offset::STREAM_CHANNEL_0_IP, u32::from(Ipv4Addr::LOCALHOST));
    camera.poke_register(offset::STREAM_CHANNEL_0_PORT, u32::from(port));
    camera.poke_register(feature::WIDTH, 8);
    camera.poke_register(feature::HEIGHT, 4);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);
    receiver
}

/// The arrival times of leader packets on `receiver` during `window`.
fn leaders_during(receiver: &UdpSocket, window: Duration) -> Vec<Instant> {
    let end = Instant::now() + window;
    let mut buf = [0u8; 9000];
    let mut leaders = Vec::new();
    while Instant::now() < end {
        let Ok((n, _)) = receiver.recv_from(&mut buf) else {
            continue;
        };
        if let Ok((_, header, _)) = GvspHeader::parse(&buf[..n]) {
            if header.content_type() == ContentType::Leader {
                leaders.push(Instant::now());
            }
        }
    }
    leaders
}

fn drain(receiver: &UdpSocket) {
    receiver
        .set_read_timeout(Some(Duration::from_millis(1)))
        .unwrap();
    let mut buf = [0u8; 9000];
    while receiver.recv_from(&mut buf).is_ok() {}
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
}

#[test]
fn gvcp_and_gvsp_bind_to_the_configured_address() {
    let camera = FakeCamera::start(FakeCameraConfig {
        gvcp_bind: SocketAddrV4::new(CAMERA_IP, 0),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(*camera.local_addr().ip(), CAMERA_IP);
    assert_eq!(
        camera.peek_register(offset::CURRENT_IP),
        u32::from(CAMERA_IP)
    );
    let ack = discovery_ack(Ipv4Addr::LOCALHOST, camera.local_addr()).expect("an answer");
    assert_eq!(ack.current_ip(), CAMERA_IP);

    // Stream packets leave from the camera's IP.
    let receiver = stream_to_raw_socket(&camera);
    let mut buf = [0u8; 9000];
    let (_, from) = receiver.recv_from(&mut buf).expect("a stream packet");
    assert_eq!(from.ip(), CAMERA_IP);

    // And a client streams from it as from any other camera.
    camera.poke_register(feature::ACQUISITION_ACTIVE, 0);
    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();
    let buf = rx.recv_timeout(Duration::from_secs(3)).expect("a frame");
    cam.stop_stream(stream).unwrap();
    assert_eq!(buf.status, BufferStatus::Success);
}

#[test]
fn a_prebound_socket_and_an_explicit_stream_source_are_used() {
    let socket = UdpSocket::bind((CAMERA_IP, 0)).unwrap();
    let bound = socket.local_addr().unwrap();
    let camera = FakeCamera::start_on(
        FakeCameraConfig {
            gvsp_source_ip: Some(Ipv4Addr::new(127, 0, 0, 3)),
            ..Default::default()
        },
        socket,
    )
    .unwrap();
    assert_eq!(SocketAddr::V4(camera.local_addr()), bound);
    let ack = discovery_ack(Ipv4Addr::LOCALHOST, camera.local_addr()).expect("an answer");
    assert_eq!(ack.current_ip(), CAMERA_IP);

    let receiver = stream_to_raw_socket(&camera);
    let mut buf = [0u8; 9000];
    let (_, from) = receiver.recv_from(&mut buf).expect("a stream packet");
    assert_eq!(from.ip(), Ipv4Addr::new(127, 0, 0, 3));
}

#[test]
fn discovery_is_answered_only_from_the_cameras_subnet() {
    let camera = FakeCamera::start(FakeCameraConfig {
        gvcp_bind: SocketAddrV4::new(CAMERA_IP, 0),
        discovery_netmask: Some(Ipv4Addr::new(255, 255, 255, 254)),
        ..Default::default()
    })
    .unwrap();
    // 127.0.0.2/31 holds 127.0.0.2 and 127.0.0.3, not 127.0.0.1.
    assert!(discovery_ack(Ipv4Addr::new(127, 0, 0, 3), camera.local_addr()).is_some());
    assert!(discovery_ack(Ipv4Addr::LOCALHOST, camera.local_addr()).is_none());

    // Without a netmask, today's behaviour: everyone gets an answer.
    let open = FakeCamera::start(FakeCameraConfig {
        gvcp_bind: SocketAddrV4::new(CAMERA_IP, 0),
        ..Default::default()
    })
    .unwrap();
    assert!(discovery_ack(Ipv4Addr::LOCALHOST, open.local_addr()).is_some());
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeenRequest {
    width: u32,
    height: u32,
    offset_x: u32,
    offset_y: u32,
    pixel_format: u32,
    pixel_format_name: Option<String>,
    frame_id: u64,
    timestamp_ns: u64,
}

impl From<&FrameRequest<'_>> for SeenRequest {
    fn from(r: &FrameRequest<'_>) -> Self {
        Self {
            width: r.width,
            height: r.height,
            offset_x: r.offset_x,
            offset_y: r.offset_y,
            pixel_format: r.pixel_format,
            pixel_format_name: r.pixel_format_name.map(str::to_string),
            frame_id: r.frame_id,
            timestamp_ns: r.timestamp_ns,
        }
    }
}

/// A source whose frames are `frame_id` repeated, so every byte tells which frame it came from.
fn frame_id_source(
    seen: Arc<Mutex<Vec<SeenRequest>>>,
) -> impl FnMut(&FrameRequest<'_>) -> Option<Vec<u8>> + Send {
    move |request: &FrameRequest<'_>| {
        seen.lock().unwrap().push(request.into());
        let len = (request.width * request.height * 2) as usize;
        Some(
            (request.frame_id as u16)
                .to_le_bytes()
                .into_iter()
                .cycle()
                .take(len)
                .collect(),
        )
    }
}

#[test]
fn frames_come_from_a_custom_source_with_the_requested_region() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(15),
        ..Default::default()
    })
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    camera.set_frame_source(frame_id_source(seen.clone()));
    camera.poke_register(feature::WIDTH, 40);
    camera.poke_register(feature::HEIGHT, 20);
    camera.poke_register(feature::OFFSET_X, 6);
    camera.poke_register(feature::OFFSET_Y, 4);
    camera.poke_register(feature::PIXEL_FORMAT, pixel_format::MONO16);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();
    let buf = rx.recv_timeout(Duration::from_secs(3)).expect("a frame");
    cam.stop_stream(stream).unwrap();

    assert_eq!(buf.status, BufferStatus::Success);
    let id = buf.frame_id as u16;
    assert!(buf
        .data()
        .chunks_exact(2)
        .all(|c| u16::from_le_bytes([c[0], c[1]]) == id));

    let seen = seen.lock().unwrap();
    let request = seen
        .iter()
        .find(|r| r.frame_id as u16 == id)
        .expect("the source was asked for the frame");
    assert_eq!(
        (
            request.width,
            request.height,
            request.offset_x,
            request.offset_y
        ),
        (40, 20, 6, 4)
    );
    assert_eq!(request.pixel_format, pixel_format::MONO16);
    assert_eq!(request.pixel_format_name.as_deref(), Some("Mono16"));
    assert!(request.timestamp_ns > 0);
}

#[test]
fn a_source_that_has_no_frame_sends_nothing() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(5),
        ..Default::default()
    })
    .unwrap();
    camera.set_frame_source(|_: &FrameRequest<'_>| None);
    let receiver = stream_to_raw_socket(&camera);
    receiver
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut buf = [0u8; 9000];
    assert!(receiver.recv_from(&mut buf).is_err(), "no packets");
}

#[test]
fn the_source_can_be_swapped_while_streaming() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(10),
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::WIDTH, 16);
    camera.poke_register(feature::HEIGHT, 8);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();
    let first = rx.recv_timeout(Duration::from_secs(3)).expect("a frame");
    assert_eq!(first.status, BufferStatus::Success);
    assert!(first.data().iter().any(|&b| b != 0xab), "the gradient");

    camera.set_frame_source(|r: &FrameRequest<'_>| Some(vec![0xab; (r.width * r.height) as usize]));
    let swapped = (0..50)
        .filter_map(|_| rx.recv_timeout(Duration::from_secs(3)).ok())
        .any(|buf| buf.status == BufferStatus::Success && buf.data().iter().all(|&b| b == 0xab));
    assert!(swapped, "frames from the new source arrive");

    // And back to the built-in patterns.
    camera.set_frame_source(PatternSource);
    let back = (0..50)
        .filter_map(|_| rx.recv_timeout(Duration::from_secs(3)).ok())
        .any(|buf| buf.status == BufferStatus::Success && buf.data().iter().any(|&b| b != 0xab));
    cam.stop_stream(stream).unwrap();
    assert!(back);
}

#[test]
fn resends_replay_the_frames_the_source_produced() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(20),
        gvsp_loss_probability: 0.15,
        ..Default::default()
    })
    .unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    camera.set_frame_source(frame_id_source(seen.clone()));
    camera.poke_register(feature::WIDTH, 256);
    camera.poke_register(feature::HEIGHT, 64);
    camera.poke_register(feature::PIXEL_FORMAT, pixel_format::MONO16);

    let cam = Camera::connect_addr(camera.local_addr()).unwrap();
    let (tx, rx) = mpsc::channel();
    let stream = cam
        .start_stream(move |buffer: Buffer| {
            let _ = tx.send(buffer);
        })
        .unwrap();
    let mut complete = 0;
    for _ in 0..20 {
        let Ok(buf) = rx.recv_timeout(Duration::from_secs(3)) else {
            continue;
        };
        if buf.status != BufferStatus::Success {
            continue;
        }
        let id = buf.frame_id as u16;
        assert!(
            buf.data()
                .chunks_exact(2)
                .all(|c| u16::from_le_bytes([c[0], c[1]]) == id),
            "frame {id} holds its own bytes"
        );
        complete += 1;
    }
    cam.stop_stream(stream).unwrap();
    assert!(complete >= 10, "{complete}/20 complete under 15% loss");

    // The source was asked once per frame: resends didn't call it again.
    let seen = seen.lock().unwrap();
    let mut ids: Vec<u64> = seen.iter().map(|r| r.frame_id).collect();
    ids.dedup();
    assert_eq!(ids.len(), seen.len());
}

#[test]
fn frames_are_paced_from_acquisition_frame_rate() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(500),
        pace_from_frame_rate: true,
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::FRAME_RATE, 50.0f32.to_bits());
    let receiver = stream_to_raw_socket(&camera);

    let fast = leaders_during(&receiver, Duration::from_secs(1)).len();
    assert!((35..=60).contains(&fast), "{fast} frames at 50 Hz");

    camera.poke_register(feature::FRAME_RATE, 10.0f32.to_bits());
    drain(&receiver);
    let slow = leaders_during(&receiver, Duration::from_secs(1)).len();
    assert!((6..=13).contains(&slow), "{slow} frames at 10 Hz");

    // With the enable off, frame_period applies again.
    camera.poke_register(feature::FRAME_RATE_ENABLE, 0);
    drain(&receiver);
    let fallback = leaders_during(&receiver, Duration::from_secs(1)).len();
    assert!((1..=3).contains(&fallback), "{fallback} frames at 2 Hz");
}

#[test]
fn without_pacing_the_frame_rate_registers_are_only_stored() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(10),
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::FRAME_RATE, 2.0f32.to_bits());
    let receiver = stream_to_raw_socket(&camera);
    let frames = leaders_during(&receiver, Duration::from_millis(500)).len();
    assert!(frames >= 25, "{frames} frames at the 10 ms period");
}

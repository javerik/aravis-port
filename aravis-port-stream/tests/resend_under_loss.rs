use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use aravis_port_core::bootstrap::offset;
use aravis_port_core::gvcp::PacketResend;
use aravis_port_core::memory::{new_buffer_pool, BufferStatus};
use aravis_port_device::{Device, DeviceConfig, PacketResendSender};
use aravis_port_fakecamera::{feature, FakeCamera, FakeCameraConfig};
use aravis_port_stream::{spawn, ResendRequester, StreamConfig};

struct DeviceRequester(PacketResendSender);
impl ResendRequester for DeviceRequester {
    fn request_resend(&mut self, resend: PacketResend) {
        let _ = self.0.send(resend);
    }
}

/// With a real GVCP-connected resend requester (not a no-op) and the fake camera dropping a
/// meaningful fraction of packets, frames should still complete successfully via resend — this
/// is the same reassembly/resend path validated against real hardware in Phase 4, now exercised
/// under controlled, repeatable packet loss.
#[test]
fn frames_complete_successfully_despite_packet_loss_via_resend() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(20),
        gvsp_loss_probability: 0.15,
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::WIDTH, 256);
    camera.poke_register(feature::HEIGHT, 256);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let device = Device::connect(
        camera.local_addr(),
        DeviceConfig {
            heartbeat_period: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .unwrap();

    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let local_port = match socket.local_addr().unwrap() {
        SocketAddr::V4(v4) => v4.port(),
        _ => unreachable!(),
    };
    device
        .open_stream_channel(Ipv4Addr::LOCALHOST, local_port)
        .unwrap();
    device
        .write_register(offset::STREAM_CHANNEL_0_IP, u32::from(Ipv4Addr::LOCALHOST))
        .unwrap();

    let (pool_user, pool_stream) = new_buffer_pool(4, 256 * 256);
    let requester = Box::new(DeviceRequester(device.resend_sender()));
    let handle = spawn(
        socket,
        StreamConfig::default(),
        pool_stream,
        requester,
        None,
    )
    .unwrap();

    let mut successes = 0;
    let mut total = 0;
    for _ in 0..15 {
        if let Some(buf) = pool_user.timeout_pop_buffer(Duration::from_secs(3)) {
            total += 1;
            if buf.status == BufferStatus::Success {
                successes += 1;
                assert_eq!(buf.data().len(), 256 * 256);
            }
            pool_user.push_buffer(buf);
        }
    }

    handle.stop();
    assert!(
        total >= 10,
        "expected at least 10 frame outcomes, got {total}"
    );
    assert!(
        successes as f64 / total as f64 >= 0.7,
        "expected resend to recover most frames despite 15% packet loss: {successes}/{total} succeeded"
    );
}

/// A device that has already dropped the packets a resend asks for answers with data-less
/// "packet unavailable" error packets. Those frames must close as incomplete: one that closed as
/// Success would be missing whatever was lost (seen on the live C6 as a frame one packet short).
#[test]
fn frames_whose_lost_packets_are_unavailable_never_close_as_success() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(10),
        gvsp_loss_probability: 0.2,
        resend_unavailable: true,
        ..Default::default()
    })
    .unwrap();
    let (width, height) = (64u32, 64u32);
    camera.poke_register(feature::WIDTH, width);
    camera.poke_register(feature::HEIGHT, height);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let device = Device::connect(
        camera.local_addr(),
        DeviceConfig {
            heartbeat_period: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let local_port = match socket.local_addr().unwrap() {
        SocketAddr::V4(v4) => v4.port(),
        _ => unreachable!(),
    };
    device
        .open_stream_channel(Ipv4Addr::LOCALHOST, local_port)
        .unwrap();
    device
        .write_register(offset::STREAM_CHANNEL_0_IP, u32::from(Ipv4Addr::LOCALHOST))
        .unwrap();

    let (pool_user, pool_stream) = new_buffer_pool(4, (width * height) as usize);
    let requester = Box::new(DeviceRequester(device.resend_sender()));
    let handle = spawn(
        socket,
        StreamConfig::default(),
        pool_stream,
        requester,
        None,
    )
    .unwrap();

    let (mut successes, mut incomplete) = (0, 0);
    for _ in 0..40 {
        let Some(buf) = pool_user.timeout_pop_buffer(Duration::from_secs(3)) else {
            continue;
        };
        if buf.status == BufferStatus::Success {
            successes += 1;
            // The fake's Mono8 gradient, so a zero-filled gap fails as well as a short tail.
            let expected: Vec<u8> = (0..height)
                .flat_map(|y| (0..width).map(move |x| ((x + y + buf.frame_id as u32) % 255) as u8))
                .collect();
            assert!(
                buf.data() == expected.as_slice(),
                "frame {} closed as Success with wrong data",
                buf.frame_id
            );
            assert!(
                buf.image.is_some(),
                "frame {} closed as Success without its leader",
                buf.frame_id
            );
        } else {
            incomplete += 1;
        }
        pool_user.push_buffer(buf);
    }

    handle.stop();
    assert!(
        successes > 0,
        "no frame arrived without loss ({incomplete} incomplete)"
    );
    assert!(
        incomplete > 0,
        "no frame lost a packet ({successes} complete)"
    );
}

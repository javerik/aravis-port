use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use aravis_port_core::bootstrap::offset;
use aravis_port_core::gvcp::PacketResend;
use aravis_port_fakecamera::{feature, FakeCamera, FakeCameraConfig};
use aravis_port_core::memory::{new_buffer_pool, BufferStatus};
use aravis_port_stream::{spawn, ResendRequester, StreamConfig};

struct NoopRequester;
impl ResendRequester for NoopRequester {
    fn request_resend(&mut self, _: PacketResend) {}
}

fn point_camera_stream_channel_at(camera: &FakeCamera, socket: &UdpSocket) {
    let local_addr = socket.local_addr().unwrap();
    let port = match local_addr {
        SocketAddr::V4(v4) => v4.port(),
        _ => unreachable!("bound to an IPv4 loopback address"),
    };
    camera.poke_register(offset::STREAM_CHANNEL_0_IP, u32::from(Ipv4Addr::LOCALHOST));
    camera.poke_register(offset::STREAM_CHANNEL_0_PORT, port as u32);
}

#[test]
fn receives_ten_frames_from_the_fake_camera() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(10),
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::WIDTH, 16);
    camera.poke_register(feature::HEIGHT, 16);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    point_camera_stream_channel_at(&camera, &socket);

    let (pool_user, pool_stream) = new_buffer_pool(4, 16 * 16);
    let handle = spawn(socket, StreamConfig::default(), pool_stream, Box::new(NoopRequester), None).unwrap();

    let mut received = 0;
    for _ in 0..10 {
        let buf = pool_user
            .timeout_pop_buffer(Duration::from_secs(2))
            .expect("expected a frame within 2 seconds");
        assert_eq!(buf.status, BufferStatus::Success);
        assert_eq!(buf.data().len(), 16 * 16);
        assert_eq!(buf.image.unwrap().width, 16);
        pool_user.push_buffer(buf);
        received += 1;
    }

    handle.stop();
    assert_eq!(received, 10);
}

#[test]
fn frame_ids_are_monotonically_increasing() {
    let camera = FakeCamera::start(FakeCameraConfig {
        frame_period: Duration::from_millis(5),
        ..Default::default()
    })
    .unwrap();
    camera.poke_register(feature::WIDTH, 8);
    camera.poke_register(feature::HEIGHT, 8);
    camera.poke_register(feature::ACQUISITION_ACTIVE, 1);

    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    point_camera_stream_channel_at(&camera, &socket);

    let (pool_user, pool_stream) = new_buffer_pool(4, 8 * 8);
    let handle = spawn(socket, StreamConfig::default(), pool_stream, Box::new(NoopRequester), None).unwrap();

    let mut last_frame_id = None;
    for _ in 0..5 {
        let buf = pool_user.timeout_pop_buffer(Duration::from_secs(2)).unwrap();
        if let Some(last) = last_frame_id {
            assert!(buf.frame_id > last, "frame ids must increase: {} then {}", last, buf.frame_id);
        }
        last_frame_id = Some(buf.frame_id);
        pool_user.push_buffer(buf);
    }
    handle.stop();
}

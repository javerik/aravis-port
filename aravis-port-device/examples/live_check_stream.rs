//! Phase-4 live-hardware check: open a GVSP stream channel against the real camera, start
//! acquisition, and receive real frames end-to-end through `aravis-port-stream`'s reassembler.
//!
//! `cargo run -p aravis-port-device --example live_check_stream -- 169.254.133.91 169.254.1.1`

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Duration;

use aravis_port_core::gvcp::PacketResend;
use aravis_port_device::{Device, DeviceConfig};
use aravis_port_memory::new_buffer_pool;
use aravis_port_stream::{spawn, ResendRequester, StreamConfig};

/// No resend support in this smoke test (a direct GigE link shouldn't drop packets over a few
/// frames); logs if the reassembler ever asks for one so packet loss would still be visible.
struct LoggingRequester;
impl ResendRequester for LoggingRequester {
    fn request_resend(&mut self, resend: PacketResend) {
        eprintln!("(resend requested but not implemented in this smoke test: {resend:?})");
    }
}

fn main() {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let camera_ip: Ipv4Addr = args.next().expect("usage: live_check_stream <camera-ip> <local-ip>").parse().unwrap();
    let local_ip: Ipv4Addr = args.next().expect("usage: live_check_stream <camera-ip> <local-ip>").parse().unwrap();

    let device = Device::connect(SocketAddrV4::new(camera_ip, aravis_port_core::gvcp::PORT), DeviceConfig::default())
        .expect("connect failed");

    let width = device.read::<i64>("Width").unwrap();
    let height = device.read::<i64>("Height").unwrap();
    let payload_size = device.read::<i64>("PayloadSize").unwrap();
    println!("Width={width} Height={height} PayloadSize={payload_size}");

    let socket = UdpSocket::bind((local_ip, 0)).expect("bind local GVSP socket failed");
    let local_port = match socket.local_addr().unwrap() {
        SocketAddr::V4(v4) => v4.port(),
        _ => unreachable!(),
    };
    println!("local GVSP socket: {local_ip}:{local_port}");

    device
        .open_stream_channel(local_ip, local_port)
        .expect("failed to write GevSCDA/GevSCPHostPort");

    // The device's power-on GevSCPSPacketSize can exceed the local link's MTU once IP/UDP
    // headers are added; pin it to a safely-under-1500-MTU value we also use for reassembly.
    const PACKET_SIZE: u16 = 1400;
    device.set_stream_packet_size(PACKET_SIZE).expect("failed to set GevSCPSPacketSize");
    println!("stream channel packet size (GevSCPSPacketSize) = {:?}", device.stream_packet_size());

    let scda = device.read_register(0xd18).unwrap();
    let scp_host_port = device.read_register(0xd00).unwrap();
    println!(
        "read back: GevSCDA=0x{scda:08x} ({}) GevSCPHostPort=0x{scp_host_port:08x} (port={})",
        std::net::Ipv4Addr::from(scda),
        scp_host_port & 0xffff
    );

    let (pool_user, pool_stream) = new_buffer_pool(4, payload_size as usize);
    let cfg = StreamConfig {
        packet_size: PACKET_SIZE,
        ..StreamConfig::default()
    };
    let handle = spawn(socket, cfg, pool_stream, Box::new(LoggingRequester), None).expect("failed to spawn receiver");

    println!("AcquisitionStatus before start = {:?}", device.read::<i64>("AcquisitionStatus"));
    device.execute_command("AcquisitionStart").expect("AcquisitionStart failed");
    std::thread::sleep(Duration::from_millis(200));
    println!("AcquisitionStatus after start = {:?}", device.read::<i64>("AcquisitionStatus"));
    println!("AcquisitionStartReg (0xD314) read back = {:?}", device.read_register(0xD314));

    let mut received = 0;
    for i in 0..10 {
        match pool_user.timeout_pop_buffer(Duration::from_secs(3)) {
            Some(buf) => {
                println!(
                    "frame {i}: id={} status={:?} bytes={} image={:?}",
                    buf.frame_id,
                    buf.status,
                    buf.data().len(),
                    buf.image
                );
                pool_user.push_buffer(buf);
                received += 1;
            }
            None => {
                println!("frame {i}: TIMED OUT waiting for a frame");
                break;
            }
        }
    }

    device.execute_command("AcquisitionStop").ok();
    handle.stop();
    println!("received {received}/10 frames");
}

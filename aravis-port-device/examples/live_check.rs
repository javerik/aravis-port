//! Phase-2 live-hardware check: connect to the real camera, read/write safe registers, and
//! observe the heartbeat keeping control-channel privilege alive.
//!
//! `cargo run -p aravis-port-device --example live_check -- 169.254.133.91`

use std::net::SocketAddrV4;
use std::time::Duration;

use aravis_port_device::{Device, DeviceConfig};

fn main() {
    env_logger::init();
    let ip: std::net::Ipv4Addr = std::env::args()
        .nth(1)
        .expect("usage: live_check <camera-ip>")
        .parse()
        .expect("invalid ip");
    let peer = SocketAddrV4::new(ip, aravis_port_core::gvcp::PORT);

    let device = Device::connect(peer, DeviceConfig::default()).expect("connect failed");
    println!("connected, has_control={}", device.has_control());

    // GevVersion register (0x0) is always safe to read on any GEV device.
    let version = device.read_register(aravis_port_core::bootstrap::offset::VERSION).unwrap();
    println!("GevVersion register = 0x{version:08x}");

    // Read the current heartbeat timeout (safe, read-only check).
    let heartbeat_timeout = device
        .read_register(aravis_port_core::bootstrap::offset::HEARTBEAT_TIMEOUT)
        .unwrap();
    println!("GevHeartbeatTimeout = {heartbeat_timeout} ms");

    std::thread::sleep(Duration::from_millis(2500));
    println!("after 2.5s, has_control={} (heartbeat should have kept it alive)", device.has_control());
}

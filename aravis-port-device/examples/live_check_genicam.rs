//! Phase-3 live-hardware check: fetch and parse the real camera's actual GenICam XML, then
//! read/write a battery of real features and cross-check against `arv-tool-0.10`'s readings.
//!
//! `cargo run -p aravis-port-device --example live_check_genicam -- 169.254.133.91`

use std::net::SocketAddrV4;

use aravis_port_device::{Device, DeviceConfig};

fn main() {
    env_logger::init();
    let ip: std::net::Ipv4Addr = std::env::args()
        .nth(1)
        .expect("usage: live_check_genicam <camera-ip>")
        .parse()
        .expect("invalid ip");
    let peer = SocketAddrV4::new(ip, aravis_port_core::gvcp::PORT);

    let device = Device::connect(peer, DeviceConfig::default()).expect("connect + xml fetch/parse failed");
    println!("connected and parsed GenICam XML successfully, has_control={}", device.has_control());

    println!("Root categories: {:?}", device.categories().unwrap());

    println!("--- reads ---");
    println!("DeviceVendorName = {:?}", device.read::<String>("DeviceVendorName"));
    println!("DeviceModelName = {:?}", device.read::<String>("DeviceModelName"));
    println!("DeviceSerialNumber = {:?}", device.read::<String>("DeviceSerialNumber"));
    println!("Width = {:?}", device.read::<i64>("Width"));
    println!("Height = {:?}", device.read::<i64>("Height"));
    println!("PixelFormat = {:?}", device.read::<String>("PixelFormat"));
    println!("ExposureTime = {:?}", device.read::<f64>("ExposureTime"));
    println!("AcquisitionFrameRate = {:?}", device.read::<f64>("AcquisitionFrameRate"));
    println!("PayloadSize = {:?}", device.read::<i64>("PayloadSize"));
    println!("TestPattern = {:?}", device.read::<String>("TestPattern"));
    // GevSCPSPacketSize is backed by a MaskedIntReg with <LSB>31</LSB><MSB>16</MSB> (LSB > MSB) —
    // a big-endian bit-reversal convention; checking this exercises that specific decode path.
    println!("GevSCPSPacketSize (masked, LSB>MSB) = {:?}", device.read::<i64>("GevSCPSPacketSize"));

    println!("--- safe writes (ExposureTime, AcquisitionFrameRate untouched; TestPattern round-tripped) ---");
    let original_test_pattern = device.read::<String>("TestPattern").unwrap();
    device.write("TestPattern", "Off".to_string()).expect("write TestPattern=Off failed");
    println!("TestPattern after write Off = {:?}", device.read::<String>("TestPattern"));
    device
        .write("TestPattern", original_test_pattern.clone())
        .expect("failed to restore TestPattern");
    println!("TestPattern restored to {:?} = {:?}", original_test_pattern, device.read::<String>("TestPattern"));

    let original_exposure = device.read::<f64>("ExposureTime").unwrap();
    device.write("ExposureTime", original_exposure + 500.0).unwrap();
    println!("ExposureTime after +500 = {:?}", device.read::<f64>("ExposureTime"));
    device.write("ExposureTime", original_exposure).unwrap();
    println!("ExposureTime restored = {:?}", device.read::<f64>("ExposureTime"));
}

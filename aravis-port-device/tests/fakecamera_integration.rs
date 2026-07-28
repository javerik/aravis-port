use std::time::Duration;

use aravis_port_core::bootstrap::offset;
use aravis_port_core::Error;
use aravis_port_device::{Device, DeviceConfig};
use aravis_port_fakecamera::{feature, FakeCamera, FakeCameraConfig};
use aravis_port_device::net::{GvcpTransaction, TransactionConfig};

fn fast_device_config() -> DeviceConfig {
    DeviceConfig {
        transaction: TransactionConfig {
            retries: 2,
            timeout: Duration::from_millis(200),
        },
        heartbeat_period: Duration::from_millis(30),
    }
}

#[test]
fn connect_acquires_control_and_reads_feature_registers() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();

    assert!(device.has_control());
    assert_eq!(camera.controller().unwrap().ip().to_string(), "127.0.0.1");

    // Defaults set by RegisterBank::new.
    assert_eq!(device.read_register(feature::WIDTH).unwrap(), 64);
    assert_eq!(device.read_register(feature::EXPOSURE_TIME_US).unwrap(), 10_000);
}

#[test]
fn write_register_round_trips_through_the_wire() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();

    device.write_register(feature::EXPOSURE_TIME_US, 25_000).unwrap();
    assert_eq!(device.read_register(feature::EXPOSURE_TIME_US).unwrap(), 25_000);
    assert_eq!(camera.peek_register(feature::EXPOSURE_TIME_US), 25_000);
}

#[test]
fn read_memory_fetches_the_served_xml_byte_for_byte() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();

    let expected = camera.xml_bytes();
    let fetched = device
        .read_memory(aravis_port_fakecamera::REGISTER_SPACE_SIZE as u32, expected.len())
        .unwrap();
    assert_eq!(fetched, expected);
}

#[test]
fn heartbeat_thread_detects_out_of_band_control_loss() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();
    assert!(device.has_control());

    // Simulate the device silently losing control (e.g. another controller took over).
    camera.poke_register(offset::CONTROL_CHANNEL_PRIVILEGE, 0);

    std::thread::sleep(Duration::from_millis(150));
    assert!(!device.has_control());
}

#[test]
fn genicam_feature_read_write_and_enum_and_command_through_the_full_stack() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();

    // Float feature backed by a register (ExposureTimeReg).
    assert_eq!(device.read::<f64>("ExposureTime").unwrap(), 10_000.0);
    device.write("ExposureTime", 20_000.0).unwrap();
    assert_eq!(device.read::<f64>("ExposureTime").unwrap(), 20_000.0);
    assert_eq!(camera.peek_register(feature::EXPOSURE_TIME_US), 20_000);

    // Integer feature backed by a register (GainReg).
    device.write("Gain", 5i64).unwrap();
    assert_eq!(device.read::<i64>("Gain").unwrap(), 5);

    // Enumeration read/write by symbolic name.
    assert_eq!(device.read::<String>("PixelFormat").unwrap(), "Mono8");
    device.write("PixelFormat", "Mono16".to_string()).unwrap();
    assert_eq!(device.read::<String>("PixelFormat").unwrap(), "Mono16");
    assert_eq!(camera.peek_register(feature::PIXEL_FORMAT), aravis_port_fakecamera::pixel_format::MONO16);

    // Command execution writes CommandValue into the linked register.
    device.execute_command("AcquisitionStart").unwrap();
    assert_eq!(camera.peek_register(feature::ACQUISITION_ACTIVE), 1);

    // Root category lists the expected top-level features.
    let categories = device.categories().unwrap();
    for expected in ["Width", "Height", "PixelFormat", "ExposureTime", "Gain", "AcquisitionStart"] {
        assert!(categories.iter().any(|c| c == expected), "missing {expected} in {categories:?}");
    }
}

#[test]
fn non_controller_writes_are_rejected() {
    let camera = FakeCamera::start(FakeCameraConfig::default()).unwrap();
    let _device = Device::connect(camera.local_addr(), fast_device_config()).unwrap();

    // A second, independent client that never acquired control-channel privilege.
    let mut intruder = GvcpTransaction::connect(
        camera.local_addr(),
        TransactionConfig {
            retries: 1,
            timeout: Duration::from_millis(200),
        },
    )
    .unwrap();

    let result = intruder.write_register(feature::WIDTH, 999);
    assert!(matches!(result, Err(Error::GvcpError(_))));
    // The privileged device's earlier write is unaffected.
    assert_eq!(camera.peek_register(feature::WIDTH), 64);
}

use std::collections::HashMap;

use aravis_port_core::bootstrap::{control_channel_privilege, offset};
use aravis_port_core::MacAddress;

/// Size of the GVBS bootstrap register block (`0x0000..0x1000`). In built-in mode the XML is
/// served immediately after this block (`0x1000`); in vendor-XML mode the XML is served at a
/// configurable address (defaulting to `0x0100_0000`) and all other 32-bit addresses outside the
/// XML slice are backed by sparse memory.
pub const REGISTER_SPACE_SIZE: usize = 0x1000;

/// Custom feature registers, placed in the unused gap between the standard identity block
/// (ending at `DISCOVERY_DATA_SIZE` = 0xf8) and `XML_URL_0` (0x200) — the same trick the real
/// Aravis fake camera uses.
pub mod feature {
    pub const WIDTH: u32 = 0x100;
    pub const HEIGHT: u32 = 0x104;
    pub const PIXEL_FORMAT: u32 = 0x108;
    pub const EXPOSURE_TIME_US: u32 = 0x10c;
    pub const GAIN_RAW: u32 = 0x110;
    pub const ACQUISITION_ACTIVE: u32 = 0x114;
    pub const CHUNK_MODE_ACTIVE: u32 = 0x118;
    /// `AcquisitionFrameRateEnable` (0/1). With `FakeCameraConfig::pace_from_frame_rate`, a set
    /// enable paces frames from [`FRAME_RATE`]; otherwise it is only stored.
    pub const FRAME_RATE_ENABLE: u32 = 0x11c;
    /// `AcquisitionFrameRate` in Hz, an IEEE-754 `f32` bit pattern. Only used for pacing, like
    /// the enable.
    pub const FRAME_RATE: u32 = 0x120;
    /// `TriggerSoftware`'s write-only target; holds the last command value written.
    pub const TRIGGER_SOFTWARE: u32 = 0x124;
    /// `ReverseX` (0/1). Only stored.
    pub const REVERSE_X: u32 = 0x128;
    /// `OffsetX`/`OffsetY`: where the region sits on the sensor. Only stored: the pattern is
    /// generated for the region's size alone.
    pub const OFFSET_X: u32 = 0x12c;
    pub const OFFSET_Y: u32 = 0x130;
    /// `DeviceScanType`: [`super::scan_type`]. Switching it moves `PixelFormat` onto a format
    /// the new mode offers, the way a 3D profiler such as the AT C6 does.
    pub const DEVICE_SCAN_TYPE: u32 = 0x134;
    /// `Scan3dCoordinateSelector` (0/1/2 = CoordinateA/B/C). It selects which of the
    /// registers below `Scan3dCoordinateScale`/`Scan3dCoordinateOffset` report.
    pub const SCAN3D_COORDINATE_SELECTOR: u32 = 0x138;
    /// `Scan3dCoordinateScale` of CoordinateA/B/C: three IEEE-754 `f32` bit patterns, 4 bytes
    /// apart. Read-only over GVCP; the device's calibration, set with
    /// `FakeCamera::set_scan3d_calibration`.
    pub const SCAN3D_COORDINATE_SCALE: u32 = 0x13c;
    /// `Scan3dCoordinateOffset` of CoordinateA/B/C, laid out like [`SCAN3D_COORDINATE_SCALE`].
    pub const SCAN3D_COORDINATE_OFFSET: u32 = 0x148;
}

/// The power-on `Scan3dCoordinateScale` of CoordinateA/B/C in mm: what the fake has always
/// reported. The offsets are 0.
pub const SCAN3D_DEFAULT_SCALES: [f32; 3] = [0.05, 0.1, 0.001];

/// PixelFormat register values understood by the built-in fake-camera GenICam XML.
pub mod pixel_format {
    pub const MONO8: u32 = 1;
    pub const MONO10: u32 = 2;
    pub const MONO16: u32 = 3;
    /// A 16-bit range (z) image, offered only in [`super::scan_type::LINESCAN3D`].
    pub const COORD3D_C16: u32 = 4;

    /// The symbolic name of a `PixelFormat` value, as the built-in XML's enumeration lists it.
    pub fn name(value: u32) -> Option<&'static str> {
        match value {
            MONO8 => Some("Mono8"),
            MONO10 => Some("Mono10"),
            MONO16 => Some("Mono16"),
            COORD3D_C16 => Some("Coord3D_C16"),
            _ => None,
        }
    }
}

/// `DeviceScanType` register values.
pub mod scan_type {
    pub const AREASCAN: u32 = 0;
    /// Every frame is a stack of laser profiles: one row per profile, the value the height.
    pub const LINESCAN3D: u32 = 1;
}

/// Whether the device takes `PixelFormat = pixel_format` in scan mode `scan`: Linescan3D
/// streams nothing but `Coord3D_C16`, and area scan everything else. A device refuses the
/// others even though the GenICam client writes any entry it is asked to.
pub(crate) fn pixel_format_allowed(scan: u32, pixel_format: u32) -> bool {
    match scan {
        scan_type::LINESCAN3D => pixel_format == pixel_format::COORD3D_C16,
        scan_type::AREASCAN => pixel_format != pixel_format::COORD3D_C16,
        _ => true,
    }
}

/// The format a switch into scan mode `scan` leaves the device in.
pub(crate) fn default_pixel_format(scan: u32) -> u32 {
    if scan == scan_type::LINESCAN3D {
        pixel_format::COORD3D_C16
    } else {
        pixel_format::MONO8
    }
}

/// The fake camera's register bank plus served XML blob, addressed over the 32-bit range.
pub struct RegisterBank {
    pub registers: Vec<u8>,
    pub xml: Vec<u8>,
    pub xml_address: u32,
    sparse: HashMap<u32, u8>,
}

pub struct Identity {
    pub manufacturer: String,
    pub model: String,
    pub version: String,
    pub serial: String,
    pub mac: MacAddress,
    pub current_ip: std::net::Ipv4Addr,
}

impl RegisterBank {
    fn bootstrap_registers(
        identity: &Identity,
        xml_name: &str,
        xml_address: u32,
        xml_len: usize,
    ) -> Vec<u8> {
        let mut registers = vec![0u8; REGISTER_SPACE_SIZE];

        write_str(
            &mut registers,
            offset::MANUFACTURER_NAME,
            offset::MANUFACTURER_NAME_LEN,
            &identity.manufacturer,
        );
        write_str(
            &mut registers,
            offset::MODEL_NAME,
            offset::MODEL_NAME_LEN,
            &identity.model,
        );
        write_str(
            &mut registers,
            offset::DEVICE_VERSION,
            offset::DEVICE_VERSION_LEN,
            &identity.version,
        );
        write_str(
            &mut registers,
            offset::SERIAL_NUMBER,
            offset::SERIAL_NUMBER_LEN,
            &identity.serial,
        );

        let mac = identity.mac.octets();
        write_u32(
            &mut registers,
            offset::MAC_HIGH,
            u32::from(mac[0]) << 8 | u32::from(mac[1]),
        );
        write_u32(
            &mut registers,
            offset::MAC_LOW,
            u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]),
        );
        write_u32(
            &mut registers,
            offset::CURRENT_IP,
            u32::from(identity.current_ip),
        );

        write_u32(&mut registers, offset::N_STREAM_CHANNELS, 1);
        write_u32(&mut registers, offset::HEARTBEAT_TIMEOUT, 3000);
        write_u32(&mut registers, offset::CONTROL_CHANNEL_PRIVILEGE, 0);
        write_u32(&mut registers, offset::STREAM_CHANNEL_0_PACKET_SIZE, 1500);

        let url = format!("Local:{xml_name};{xml_address:x};{xml_len:x}");
        write_str(&mut registers, offset::XML_URL_0, offset::XML_URL_LEN, &url);
        registers
    }

    pub fn new(identity: &Identity, xml: Vec<u8>) -> Self {
        let xml_address = REGISTER_SPACE_SIZE as u32;
        let xml_name = if aravis_port_core::zip::is_zip(&xml) {
            "genicam.xml.zip"
        } else {
            "genicam.xml"
        };
        let mut registers = Self::bootstrap_registers(identity, xml_name, xml_address, xml.len());

        // Sensible feature defaults, matching the shape of a real camera's power-on state.
        write_u32(&mut registers, feature::WIDTH, 64);
        write_u32(&mut registers, feature::HEIGHT, 64);
        write_u32(&mut registers, feature::PIXEL_FORMAT, pixel_format::MONO8);
        write_u32(&mut registers, feature::EXPOSURE_TIME_US, 10_000);
        write_u32(&mut registers, feature::GAIN_RAW, 0);
        write_u32(&mut registers, feature::ACQUISITION_ACTIVE, 0);
        write_u32(&mut registers, feature::CHUNK_MODE_ACTIVE, 0);
        write_u32(&mut registers, feature::FRAME_RATE_ENABLE, 1);
        write_u32(&mut registers, feature::FRAME_RATE, 30.0f32.to_bits());
        write_u32(&mut registers, feature::TRIGGER_SOFTWARE, 0);
        write_u32(&mut registers, feature::REVERSE_X, 0);
        write_u32(&mut registers, feature::OFFSET_X, 0);
        write_u32(&mut registers, feature::OFFSET_Y, 0);
        write_u32(
            &mut registers,
            feature::DEVICE_SCAN_TYPE,
            scan_type::AREASCAN,
        );
        write_u32(&mut registers, feature::SCAN3D_COORDINATE_SELECTOR, 0);
        for (i, scale) in SCAN3D_DEFAULT_SCALES.iter().enumerate() {
            let at = 4 * i as u32;
            write_u32(
                &mut registers,
                feature::SCAN3D_COORDINATE_SCALE + at,
                scale.to_bits(),
            );
            write_u32(&mut registers, feature::SCAN3D_COORDINATE_OFFSET + at, 0);
        }

        Self {
            registers,
            xml,
            xml_address,
            sparse: HashMap::new(),
        }
    }

    /// Construct a register bank for vendor-XML mode: GVBS bootstrap registers in
    /// `0x0000..0x1000`, the raw (or `.zip`) XML served read-only at `xml_address`, and sparse
    /// memory over the rest of the 32-bit address space.
    pub fn new_vendor(
        identity: &Identity,
        xml: Vec<u8>,
        xml_address: u32,
        xml_file_name: &str,
    ) -> Self {
        let registers = Self::bootstrap_registers(identity, xml_file_name, xml_address, xml.len());
        Self {
            registers,
            xml,
            xml_address,
            sparse: HashMap::new(),
        }
    }

    /// Read `len` bytes starting at `address`. Addresses inside the GVBS bootstrap block come
    /// from `registers`, addresses in `[xml_address .. xml_address + xml.len())` come from the
    /// served XML blob, and all other 32-bit addresses read from sparse memory (defaulting to 0).
    pub fn read(&self, address: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let reg_len = self.registers.len() as u64;
        let xml_start = u64::from(self.xml_address);
        let xml_end = xml_start + self.xml.len() as u64;
        for i in 0..len {
            let a = u64::from(address) + i as u64;
            let byte = if a > u64::from(u32::MAX) {
                0
            } else if a < reg_len {
                self.registers[a as usize]
            } else if a >= xml_start && a < xml_end {
                self.xml[(a - xml_start) as usize]
            } else {
                self.sparse.get(&(a as u32)).copied().unwrap_or(0)
            };
            out.push(byte);
        }
        out
    }

    /// Write `data` at `address`. Bytes landing in the read-only XML region are silently dropped,
    /// matching a real device where the XML region isn't writable memory.
    pub fn write(&mut self, address: u32, data: &[u8]) {
        let reg_len = self.registers.len() as u64;
        let xml_start = u64::from(self.xml_address);
        let xml_end = xml_start + self.xml.len() as u64;
        for (i, &byte) in data.iter().enumerate() {
            let a = u64::from(address) + i as u64;
            if a > u64::from(u32::MAX) {
                continue;
            }
            if a >= xml_start && a < xml_end {
                continue;
            }
            if a < reg_len {
                self.registers[a as usize] = byte;
            } else if byte == 0 {
                self.sparse.remove(&(a as u32));
            } else {
                self.sparse.insert(a as u32, byte);
            }
        }
    }

    pub fn read_u32(&self, address: u32) -> u32 {
        let bytes = self.read(address, 4);
        u32::from_be_bytes(bytes.try_into().unwrap())
    }

    pub fn write_u32(&mut self, address: u32, value: u32) {
        self.write(address, &value.to_be_bytes());
    }

    pub fn discovery_ack_bytes(&self) -> [u8; offset::DISCOVERY_DATA_SIZE] {
        let mut buf = [0u8; offset::DISCOVERY_DATA_SIZE];
        buf.copy_from_slice(&self.registers[..offset::DISCOVERY_DATA_SIZE]);
        buf
    }

    pub fn has_control_flags(value: u32) -> bool {
        value & (control_channel_privilege::EXCLUSIVE | control_channel_privilege::CONTROL) != 0
    }
}

impl aravis_port_genicam::RegisterAccess for RegisterBank {
    fn read_memory(&mut self, address: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let Ok(addr) = u32::try_from(address) else {
            return Ok(vec![0u8; len]);
        };
        Ok(self.read(addr, len))
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> std::io::Result<()> {
        if let Ok(addr) = u32::try_from(address) {
            self.write(addr, data);
        }
        Ok(())
    }
}

fn write_str(buf: &mut [u8], offset: u32, max_len: usize, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(max_len);
    let start = offset as usize;
    buf[start..start + n].copy_from_slice(&bytes[..n]);
    for b in &mut buf[start + n..start + max_len] {
        *b = 0;
    }
}

fn write_u32(buf: &mut [u8], offset: u32, value: u32) {
    let start = offset as usize;
    buf[start..start + 4].copy_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> Identity {
        Identity {
            manufacturer: "aravis-port".into(),
            model: "FakeCamera".into(),
            version: "0.1".into(),
            serial: "0001".into(),
            mac: MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]),
            current_ip: std::net::Ipv4Addr::new(127, 0, 0, 2),
        }
    }

    #[test]
    fn read_spans_register_xml_boundary() {
        let xml = b"<GenApi/>".to_vec();
        let bank = RegisterBank::new(&test_identity(), xml.clone());
        let read_back = bank.read(REGISTER_SPACE_SIZE as u32, xml.len());
        assert_eq!(read_back, xml);
    }

    #[test]
    fn feature_registers_round_trip() {
        let mut bank = RegisterBank::new(&test_identity(), vec![]);
        bank.write_u32(feature::EXPOSURE_TIME_US, 20_000);
        assert_eq!(bank.read_u32(feature::EXPOSURE_TIME_US), 20_000);
    }

    #[test]
    fn writes_beyond_register_space_are_dropped_not_panicking() {
        let mut bank = RegisterBank::new(&test_identity(), vec![1, 2, 3]);
        bank.write(REGISTER_SPACE_SIZE as u32, &[9, 9, 9]);
        assert_eq!(bank.read(REGISTER_SPACE_SIZE as u32, 3), vec![1, 2, 3]);
    }

    #[test]
    fn sparse_high_32_bit_addresses_round_trip() {
        let xml = b"<RegisterDescription/>".to_vec();
        let mut bank =
            RegisterBank::new_vendor(&test_identity(), xml.clone(), 0x0200_0000, "vendor.xml");
        assert_eq!(bank.read(0x0200_0000, xml.len()), xml);

        for addr in [0x0001_0000, 0x1000_0000, 0x8000_0000, 0xffff_fffc] {
            bank.write_u32(addr, 0xdead_beef);
            assert_eq!(bank.read_u32(addr), 0xdead_beef);
            bank.write_u32(addr, 0);
            assert_eq!(bank.read_u32(addr), 0);
        }
        assert!(bank.sparse.is_empty());
    }

    #[test]
    fn each_scan_type_takes_only_its_own_pixel_formats() {
        use pixel_format::*;
        for pf in [MONO8, MONO10, MONO16] {
            assert!(pixel_format_allowed(scan_type::AREASCAN, pf));
            assert!(!pixel_format_allowed(scan_type::LINESCAN3D, pf));
        }
        assert!(pixel_format_allowed(scan_type::LINESCAN3D, COORD3D_C16));
        assert!(!pixel_format_allowed(scan_type::AREASCAN, COORD3D_C16));
    }

    #[test]
    fn a_scan_type_switch_lands_on_a_format_the_mode_takes() {
        for scan in [scan_type::AREASCAN, scan_type::LINESCAN3D] {
            assert!(pixel_format_allowed(scan, default_pixel_format(scan)));
        }
    }

    #[test]
    fn power_on_is_area_scan() {
        let bank = RegisterBank::new(&test_identity(), vec![]);
        assert_eq!(
            bank.read_u32(feature::DEVICE_SCAN_TYPE),
            scan_type::AREASCAN
        );
    }

    #[test]
    fn discovery_ack_bytes_match_identity() {
        let bank = RegisterBank::new(&test_identity(), vec![]);
        let ack = aravis_port_core::gvcp::DiscoveryAck(bank.discovery_ack_bytes());
        assert_eq!(ack.model(), "FakeCamera");
        assert_eq!(ack.mac(), test_identity().mac);
    }
}

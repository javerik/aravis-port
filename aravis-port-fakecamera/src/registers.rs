use aravis_port_core::bootstrap::{control_channel_privilege, offset};
use aravis_port_core::MacAddress;

/// Size of the flat register bank. Addresses at or beyond this are served from the XML blob
/// instead (`address - REGISTER_SPACE_SIZE`), mirroring Aravis's "registers then XML" addressing.
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
    /// `AcquisitionFrameRateEnable` (0/1). Only stored: the fake's frame period is fixed.
    pub const FRAME_RATE_ENABLE: u32 = 0x11c;
    /// `AcquisitionFrameRate`, an IEEE-754 `f32` bit pattern. Only stored, like the enable.
    pub const FRAME_RATE: u32 = 0x120;
    /// `TriggerSoftware`'s write-only target; holds the last command value written.
    pub const TRIGGER_SOFTWARE: u32 = 0x124;
    /// `ReverseX` (0/1). Only stored.
    pub const REVERSE_X: u32 = 0x128;
    /// `OffsetX`/`OffsetY`: where the region sits on the sensor. Only stored: the pattern is
    /// generated for the region's size alone.
    pub const OFFSET_X: u32 = 0x12c;
    pub const OFFSET_Y: u32 = 0x130;
}

/// PixelFormat register values understood by the built-in fake-camera GenICam XML.
pub mod pixel_format {
    pub const MONO8: u32 = 1;
    pub const MONO10: u32 = 2;
    pub const MONO16: u32 = 3;
}

/// The fake camera's register bank plus served XML blob, addressed as one flat space.
pub struct RegisterBank {
    pub registers: Vec<u8>,
    pub xml: Vec<u8>,
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
    pub fn new(identity: &Identity, xml: Vec<u8>) -> Self {
        let mut registers = vec![0u8; REGISTER_SPACE_SIZE];

        write_str(&mut registers, offset::MANUFACTURER_NAME, offset::MANUFACTURER_NAME_LEN, &identity.manufacturer);
        write_str(&mut registers, offset::MODEL_NAME, offset::MODEL_NAME_LEN, &identity.model);
        write_str(&mut registers, offset::DEVICE_VERSION, offset::DEVICE_VERSION_LEN, &identity.version);
        write_str(&mut registers, offset::SERIAL_NUMBER, offset::SERIAL_NUMBER_LEN, &identity.serial);

        let mac = identity.mac.octets();
        write_u32(&mut registers, offset::MAC_HIGH, u32::from(mac[0]) << 8 | u32::from(mac[1]));
        write_u32(
            &mut registers,
            offset::MAC_LOW,
            u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]),
        );
        write_u32(&mut registers, offset::CURRENT_IP, u32::from(identity.current_ip));

        write_u32(&mut registers, offset::N_STREAM_CHANNELS, 1);
        write_u32(&mut registers, offset::HEARTBEAT_TIMEOUT, 3000);
        write_u32(&mut registers, offset::CONTROL_CHANNEL_PRIVILEGE, 0);

        // XML URL: "Local:<name>;<hex address>;<hex size>" — address is the absolute (flat)
        // address where the XML blob starts, i.e. REGISTER_SPACE_SIZE.
        let url = format!("Local:genicam.xml;{:x};{:x}", REGISTER_SPACE_SIZE, xml.len());
        write_str(&mut registers, offset::XML_URL_0, offset::XML_URL_LEN, &url);

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

        Self { registers, xml }
    }

    /// Read `len` bytes starting at the flat address, spanning the register/XML boundary if
    /// needed. Out-of-range bytes (beyond the XML blob) read as zero rather than panicking.
    pub fn read(&self, address: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let a = address as usize + i;
            let byte = if a < self.registers.len() {
                self.registers[a]
            } else {
                let xml_offset = a - self.registers.len();
                *self.xml.get(xml_offset).unwrap_or(&0)
            };
            out.push(byte);
        }
        out
    }

    /// Write `data` at the flat address. Bytes landing in the read-only XML region are silently
    /// dropped, matching a real device where the XML region isn't writable memory.
    pub fn write(&mut self, address: u32, data: &[u8]) {
        for (i, &byte) in data.iter().enumerate() {
            let a = address as usize + i;
            if a < self.registers.len() {
                self.registers[a] = byte;
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
    fn discovery_ack_bytes_match_identity() {
        let bank = RegisterBank::new(&test_identity(), vec![]);
        let ack = aravis_port_core::gvcp::DiscoveryAck(bank.discovery_ack_bytes());
        assert_eq!(ack.model(), "FakeCamera");
        assert_eq!(ack.mac(), test_identity().mac);
    }
}

//! GVBS (GigE Vision Bootstrap Register) offsets.
//!
//! These offsets are the single source of truth used by [`crate::gvcp::DiscoveryAck`]
//! parsing, `aravis-port-device`'s stream-channel setup, and `aravis-port-fakecamera`'s register
//! bank, so the memory map only needs to be gotten right in one place.

/// Byte offsets into GVBS register space.
pub mod offset {
    pub const VERSION: u32 = 0x0000;
    pub const DEVICE_MODE: u32 = 0x0004;
    pub const MAC_HIGH: u32 = 0x0008;
    pub const MAC_LOW: u32 = 0x000c;
    pub const SUPPORTED_IP_CONFIG: u32 = 0x0010;
    pub const CURRENT_IP_CONFIG: u32 = 0x0014;
    pub const CURRENT_IP: u32 = 0x0024;
    pub const CURRENT_SUBNET_MASK: u32 = 0x0034;
    pub const CURRENT_GATEWAY: u32 = 0x0044;
    pub const MANUFACTURER_NAME: u32 = 0x0048;
    pub const MANUFACTURER_NAME_LEN: usize = 32;
    pub const MODEL_NAME: u32 = 0x0068;
    pub const MODEL_NAME_LEN: usize = 32;
    pub const DEVICE_VERSION: u32 = 0x0088;
    pub const DEVICE_VERSION_LEN: usize = 32;
    pub const MANUFACTURER_INFO: u32 = 0x00a8;
    pub const MANUFACTURER_INFO_LEN: usize = 48;
    pub const SERIAL_NUMBER: u32 = 0x00d8;
    pub const SERIAL_NUMBER_LEN: usize = 16;
    pub const USER_DEFINED_NAME: u32 = 0x00e8;
    pub const USER_DEFINED_NAME_LEN: usize = 16;

    pub const XML_URL_0: u32 = 0x0200;
    pub const XML_URL_1: u32 = 0x0400;
    pub const XML_URL_LEN: usize = 512;

    pub const N_NETWORK_INTERFACES: u32 = 0x0600;
    pub const PERSISTENT_IP_0: u32 = 0x064c;
    pub const PERSISTENT_SUBNET_0: u32 = 0x065c;
    pub const PERSISTENT_GATEWAY_0: u32 = 0x066c;

    pub const N_MESSAGE_CHANNELS: u32 = 0x0900;
    pub const N_STREAM_CHANNELS: u32 = 0x0904;
    pub const GVCP_CAPABILITY: u32 = 0x0934;
    pub const HEARTBEAT_TIMEOUT: u32 = 0x0938;
    pub const TIMESTAMP_TICK_FREQUENCY_HIGH: u32 = 0x093c;
    pub const TIMESTAMP_TICK_FREQUENCY_LOW: u32 = 0x0940;
    pub const TIMESTAMP_CONTROL: u32 = 0x0944;
    pub const TIMESTAMP_LATCH_HIGH: u32 = 0x0948;
    pub const TIMESTAMP_LATCH_LOW: u32 = 0x094c;

    pub const CONTROL_CHANNEL_PRIVILEGE: u32 = 0x0a00;

    pub const STREAM_CHANNEL_0_PORT: u32 = 0x0d00;
    pub const STREAM_CHANNEL_0_PACKET_SIZE: u32 = 0x0d04;
    pub const STREAM_CHANNEL_0_PACKET_DELAY: u32 = 0x0d08;
    pub const STREAM_CHANNEL_0_IP: u32 = 0x0d18;

    /// Total size of the region mirrored verbatim into a `DISCOVERY_ACK` payload.
    pub const DISCOVERY_DATA_SIZE: usize = 0xf8;
}

/// Bit flags within [`offset::CONTROL_CHANNEL_PRIVILEGE`].
pub mod control_channel_privilege {
    pub const EXCLUSIVE: u32 = 1 << 0;
    pub const CONTROL: u32 = 1 << 1;
}

/// Fields of [`offset::STREAM_CHANNEL_0_PACKET_SIZE`] (`GevSCPSPacketSize` and its flags).
pub mod stream_packet_size {
    /// Writing this bit makes the device send one test packet of the programmed size to the
    /// stream channel's destination (`GevSCPSFireTestPacket`).
    pub const FIRE_TEST_PACKET: u32 = 1 << 31;
    /// Sets the IP "don't fragment" flag on stream packets (`GevSCPSDoNotFragment`).
    pub const DO_NOT_FRAGMENT: u32 = 1 << 30;
    /// Multi-byte pixels are sent big-endian (`GevSCPSBigEndian`).
    pub const BIG_ENDIAN: u32 = 1 << 29;
    /// The packet size itself: the whole IP datagram, IP and UDP headers included.
    pub const SIZE_MASK: u32 = 0xffff;
}

/// Bit flags within [`offset::CURRENT_IP_CONFIG`] / [`offset::SUPPORTED_IP_CONFIG`].
pub mod ip_config {
    pub const PERSISTENT: u32 = 1 << 0;
    pub const DHCP: u32 = 1 << 1;
    pub const LLA: u32 = 1 << 2;
}

/// A read-only view over a byte slice addressed using GVBS offsets, shared by discovery-ack
/// parsing, the fake camera's register bank, and anything else that needs to read the bootstrap
/// register map.
#[derive(Clone, Copy)]
pub struct BootstrapView<'a>(pub &'a [u8]);

impl<'a> BootstrapView<'a> {
    /// Read a big-endian `u32` at `offset`. Returns 0 if out of range rather than panicking,
    /// since callers may probe optional/vendor-specific regions.
    pub fn u32_at(&self, offset: u32) -> u32 {
        let start = offset as usize;
        match self.0.get(start..start + 4) {
            Some(bytes) => u32::from_be_bytes(bytes.try_into().unwrap()),
            None => 0,
        }
    }

    /// Read a NUL-terminated (or full-length) ASCII/UTF-8 string of `len` bytes at `offset`.
    pub fn str_at(&self, offset: u32, len: usize) -> String {
        let start = offset as usize;
        let bytes = self.0.get(start..start + len).unwrap_or(&[]);
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }

    pub fn mac_address(&self) -> crate::mac::MacAddress {
        crate::mac::MacAddress::from_high_low(
            self.u32_at(offset::MAC_HIGH),
            self.u32_at(offset::MAC_LOW),
        )
    }

    pub fn current_ip(&self) -> std::net::Ipv4Addr {
        std::net::Ipv4Addr::from(self.u32_at(offset::CURRENT_IP))
    }

    pub fn manufacturer(&self) -> String {
        self.str_at(offset::MANUFACTURER_NAME, offset::MANUFACTURER_NAME_LEN)
    }

    pub fn model(&self) -> String {
        self.str_at(offset::MODEL_NAME, offset::MODEL_NAME_LEN)
    }

    pub fn version(&self) -> String {
        self.str_at(offset::DEVICE_VERSION, offset::DEVICE_VERSION_LEN)
    }

    pub fn serial(&self) -> String {
        self.str_at(offset::SERIAL_NUMBER, offset::SERIAL_NUMBER_LEN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut buf = vec![0u8; offset::DISCOVERY_DATA_SIZE];
        buf[offset::MAC_HIGH as usize + 2..offset::MAC_HIGH as usize + 4]
            .copy_from_slice(&[0x70, 0xb3]);
        buf[offset::MAC_LOW as usize..offset::MAC_LOW as usize + 4]
            .copy_from_slice(&[0xd5, 0x34, 0x5b, 0x84]);
        buf[offset::CURRENT_IP as usize..offset::CURRENT_IP as usize + 4]
            .copy_from_slice(&[169, 254, 133, 91]);
        let manuf = b"AT-Automation Technology GmbH";
        buf[offset::MANUFACTURER_NAME as usize..offset::MANUFACTURER_NAME as usize + manuf.len()]
            .copy_from_slice(manuf);
        let model = b"C5-2040-GigE";
        buf[offset::MODEL_NAME as usize..offset::MODEL_NAME as usize + model.len()]
            .copy_from_slice(model);
        let serial = b"21312821";
        buf[offset::SERIAL_NUMBER as usize..offset::SERIAL_NUMBER as usize + serial.len()]
            .copy_from_slice(serial);
        buf
    }

    #[test]
    fn parses_fields_matching_the_live_camera() {
        let data = fixture();
        let view = BootstrapView(&data);
        assert_eq!(view.mac_address().to_string(), "70:b3:d5:34:5b:84");
        assert_eq!(
            view.current_ip(),
            std::net::Ipv4Addr::new(169, 254, 133, 91)
        );
        assert_eq!(view.manufacturer(), "AT-Automation Technology GmbH");
        assert_eq!(view.model(), "C5-2040-GigE");
        assert_eq!(view.serial(), "21312821");
    }

    #[test]
    fn out_of_range_reads_do_not_panic() {
        let data = fixture();
        let view = BootstrapView(&data);
        assert_eq!(view.u32_at(0xffff_ff00), 0);
        assert_eq!(view.str_at(0xffff_ff00, 16), "");
    }
}

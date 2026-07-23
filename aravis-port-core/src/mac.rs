use std::fmt;

/// An IEEE-802 MAC address, as reported over GVCP (e.g. in a discovery ack).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    /// Build a `MacAddress` from the GVBS bootstrap `mac_high`/`mac_low` register pair.
    ///
    /// The high register carries the top 2 bytes in its low 16 bits; the low register
    /// carries the remaining 4 bytes.
    pub const fn from_high_low(mac_high: u32, mac_low: u32) -> Self {
        let hi = (mac_high & 0xffff).to_be_bytes();
        let lo = mac_low.to_be_bytes();
        Self([hi[2], hi[3], lo[0], lo[1], lo[2], lo[3]])
    }

    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for MacAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_high_low_matches_expected_octets() {
        let mac = MacAddress::from_high_low(0x0000_70b3, 0xd5345b84);
        assert_eq!(mac.octets(), [0x70, 0xb3, 0xd5, 0x34, 0x5b, 0x84]);
        assert_eq!(mac.to_string(), "70:b3:d5:34:5b:84");
    }
}

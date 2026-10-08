use std::collections::HashMap;
use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, Instant};

use aravis_port_core::gvcp::{
    self, Command, DiscoveryAck, GvcpHeader, GvcpPayload, PacketFlags, PacketType, HEADER_LEN,
};
use aravis_port_core::{Error, MacAddress, Result};

/// Options for a discovery round. `std` has no interface-enumeration API, so unlike Aravis this
/// does not auto-discover all local NICs/broadcast addresses — callers supply explicit bind
/// addresses (and, optionally, a directed broadcast address per interface).
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    pub bind_addrs: Vec<(Ipv4Addr, Option<Ipv4Addr>)>,
    /// How long to listen for acks, applied **per bind address**. A round over `n` bind
    /// addresses therefore takes up to `n * timeout`; probe them concurrently if that matters.
    pub timeout: Duration,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            bind_addrs: vec![(Ipv4Addr::UNSPECIFIED, None)],
            timeout: Duration::from_millis(1000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    pub version: String,
    pub mac: MacAddress,
    pub current_ip: Ipv4Addr,
    /// `"vendor-model-serial"`, falling back to the MAC string if serial is empty.
    pub id: String,
}

fn sanitize_id_component(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

impl DiscoveredDevice {
    fn from_ack(ack: &DiscoveryAck) -> Self {
        let manufacturer = ack.manufacturer();
        let model = ack.model();
        let mac = ack.mac();
        let mut serial = ack.serial();
        if serial.trim().is_empty() {
            serial = mac.to_string();
        }
        let id = format!(
            "{}-{}-{}",
            sanitize_id_component(&manufacturer),
            sanitize_id_component(&model),
            sanitize_id_component(&serial)
        );
        Self {
            manufacturer,
            model,
            serial,
            version: ack.version(),
            mac,
            current_ip: ack.current_ip(),
            id,
        }
    }
}

/// Broadcast a `DISCOVERY_CMD` on every configured bind address and collect `DISCOVERY_ACK`
/// responses, deduplicated by MAC address. `opts.timeout` applies to each bind address
/// separately, so every interface gets a full listening window rather than the first one
/// consuming the whole budget.
pub fn discover(opts: &DiscoveryOptions) -> Result<Vec<DiscoveredDevice>> {
    let mut found: HashMap<MacAddress, DiscoveredDevice> = HashMap::new();

    let discovery_header = GvcpHeader {
        packet_type: PacketType::Cmd,
        raw_flags: (PacketFlags::ACK_REQUIRED | PacketFlags::ALLOW_BROADCAST_ACK).bits(),
        command: Command::DiscoveryCmd,
        size: 0,
        id: 0xffff,
    };
    let packet = discovery_header.to_bytes();

    for (bind_addr, directed_broadcast) in &opts.bind_addrs {
        let socket = UdpSocket::bind((*bind_addr, 0))?;
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;

        socket.send_to(&packet, (Ipv4Addr::BROADCAST, gvcp::PORT))?;
        if let Some(dir) = directed_broadcast {
            socket.send_to(&packet, (*dir, gvcp::PORT))?;
        }

        let deadline = Instant::now() + opts.timeout;
        while Instant::now() < deadline {
            let mut buf = [0u8; 1024];
            match socket.recv_from(&mut buf) {
                Ok((n, _from)) => {
                    let Ok(header) = GvcpHeader::from_bytes(&buf[..n]) else {
                        continue;
                    };
                    if header.packet_type != PacketType::Ack
                        || header.command != Command::DiscoveryAck
                    {
                        continue;
                    }
                    if let Ok(ack) = DiscoveryAck::decode(&buf[HEADER_LEN..n]) {
                        let device = DiscoveredDevice::from_ack(&ack);
                        found.entry(device.mac).or_insert(device);
                    }
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    Ok(found.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aravis_port_core::bootstrap::offset;

    fn ack_with(model: &str, serial: &str) -> DiscoveryAck {
        let mut raw = [0u8; offset::DISCOVERY_DATA_SIZE];
        let bytes = model.as_bytes();
        raw[offset::MODEL_NAME as usize..offset::MODEL_NAME as usize + bytes.len()]
            .copy_from_slice(bytes);
        let bytes = serial.as_bytes();
        raw[offset::SERIAL_NUMBER as usize..offset::SERIAL_NUMBER as usize + bytes.len()]
            .copy_from_slice(bytes);
        DiscoveryAck(raw)
    }

    #[test]
    fn id_falls_back_to_mac_when_serial_empty() {
        let ack = ack_with("C5-2040-GigE", "");
        let device = DiscoveredDevice::from_ack(&ack);
        assert_eq!(device.serial, device.mac.to_string());
        assert!(device
            .id
            .ends_with(&sanitize_id_component(&device.mac.to_string())));
    }

    #[test]
    fn id_uses_manufacturer_model_serial() {
        let ack = ack_with("C5-2040-GigE", "21312821");
        let device = DiscoveredDevice::from_ack(&ack);
        assert!(device.id.contains("C5-2040-GigE"));
        assert!(device.id.contains("21312821"));
    }

    /// Each bind address must get its own full listening window. A single shared deadline would
    /// let the first address consume the whole budget, leaving every later interface to fire its
    /// probe and immediately give up — silently returning only the first NIC's cameras.
    #[test]
    fn timeout_applies_per_bind_address() {
        let timeout = Duration::from_millis(150);
        let one = DiscoveryOptions {
            bind_addrs: vec![(Ipv4Addr::LOCALHOST, None)],
            timeout,
        };
        let three = DiscoveryOptions {
            bind_addrs: vec![(Ipv4Addr::LOCALHOST, None); 3],
            timeout,
        };

        let start = Instant::now();
        let _ = discover(&one);
        let single = start.elapsed();

        let start = Instant::now();
        let _ = discover(&three);
        let triple = start.elapsed();

        // Three addresses should take roughly three windows, not one. Compare against the
        // measured single-address duration so the assertion doesn't depend on wall-clock
        // precision or scheduler noise.
        assert!(
            triple >= single * 2,
            "three bind addresses took {triple:?} but one took {single:?}; the deadline looks shared"
        );
    }
}

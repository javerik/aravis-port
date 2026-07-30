use std::net::SocketAddrV4;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aravis_port_core::bootstrap::{control_channel_privilege, offset};
use aravis_port_core::{Error, Result};
use aravis_port_genicam::GenApiTree;

use crate::feature::FeatureValue;
use crate::heartbeat::HeartbeatHandle;
use crate::io::GvcpTransactionIo;
use crate::net::{GvcpTransaction, TransactionConfig};
use crate::xml_fetch;

/// Configuration for [`Device::connect`].
#[derive(Debug, Clone, Copy)]
pub struct DeviceConfig {
    pub transaction: TransactionConfig,
    pub heartbeat_period: Duration,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            transaction: TransactionConfig::default(),
            heartbeat_period: Duration::from_millis(1000),
        }
    }
}

/// A connected GVCP device: shared transaction (foreground calls + heartbeat thread both use
/// it), the parsed GenICam node tree, and control-channel-privilege lifecycle.
pub struct Device {
    conn: Arc<Mutex<GvcpTransaction>>,
    genicam: GenApiTree,
    /// The device's GenICam XML as fetched, retained so callers can serve or introspect it
    /// without a second fetch. Costs a few hundred KB per connected device — the document is
    /// only reachable through bootstrap registers and is typically zip-compressed, so
    /// re-fetching it on demand is far more expensive than keeping it.
    genicam_xml: String,
    heartbeat: Option<HeartbeatHandle>,
    control_lost: Arc<AtomicBool>,
}

impl Device {
    /// Connect to `peer`, acquire control-channel privilege, fetch and parse the GenICam XML,
    /// and start the heartbeat thread.
    pub fn connect(peer: SocketAddrV4, cfg: DeviceConfig) -> Result<Self> {
        let mut txn = GvcpTransaction::connect(peer, cfg.transaction)?;
        txn.write_register(
            offset::CONTROL_CHANNEL_PRIVILEGE,
            control_channel_privilege::EXCLUSIVE | control_channel_privilege::CONTROL,
        )?;
        let xml = xml_fetch::fetch(&mut txn)?;
        let genicam_xml = String::from_utf8_lossy(&xml).into_owned();
        let genicam = GenApiTree::parse(&genicam_xml).map_err(|e| Error::GenIcam(e.to_string()))?;

        let conn = Arc::new(Mutex::new(txn));
        let control_lost = Arc::new(AtomicBool::new(false));
        let heartbeat = HeartbeatHandle::spawn(conn.clone(), cfg.heartbeat_period, control_lost.clone());
        Ok(Self {
            conn,
            genicam,
            genicam_xml,
            heartbeat: Some(heartbeat),
            control_lost,
        })
    }

    /// Run `f` with the parsed GenICam tree and a `RegisterAccess` bridge over the shared
    /// transaction.
    pub(crate) fn with_io<T>(&self, f: impl FnOnce(&GenApiTree, &mut GvcpTransactionIo) -> Result<T>) -> Result<T> {
        let mut io = GvcpTransactionIo(self.conn.clone());
        f(&self.genicam, &mut io)
    }

    /// Point the device's GVSP stream channel 0 at `local_ip:local_port`. This is a raw
    /// bootstrap-register write (`GevSCDA`/`GevSCPHostPort`), not a GenICam feature path: these
    /// standard registers are frequently absent from a vendor's own GenICam XML (confirmed on
    /// the live C5-2040-GigE camera), so going through the register map directly is both simpler
    /// and more broadly compatible than depending on the XML defining them.
    pub fn open_stream_channel(&self, local_ip: std::net::Ipv4Addr, local_port: u16) -> Result<()> {
        self.write_register(offset::STREAM_CHANNEL_0_IP, u32::from(local_ip))?;
        // GevSCPHostPort: destination port occupies the *low* 16 bits of this register on the
        // live C5-2040-GigE camera — confirmed empirically (a write with a zero low-16 is
        // silently rejected/cleared regardless of the high bits; a nonzero low-16 always
        // sticks). This contradicts the commonly-assumed "port in the high 16 bits" convention,
        // so treat that as vendor-specific rather than universal.
        self.write_register(offset::STREAM_CHANNEL_0_PORT, local_port as u32)?;
        Ok(())
    }

    /// Read `GevSCPSPacketSize`'s low 16 bits (the actual packet size; upper bits are
    /// endianness/fragmentation/test-packet flags this crate doesn't set).
    pub fn stream_packet_size(&self) -> Result<u16> {
        Ok((self.read_register(offset::STREAM_CHANNEL_0_PACKET_SIZE)? & 0xffff) as u16)
    }

    /// Set `GevSCPSPacketSize`'s low 16 bits, preserving the current flag bits (29-31). A
    /// device's power-on default can exceed the local interface's MTU once IP/UDP headers are
    /// added (confirmed on the live C5-2040-GigE: default 1501 on a 1500-MTU link), so callers
    /// should set a safely-under-MTU value before starting acquisition.
    pub fn set_stream_packet_size(&self, size: u16) -> Result<()> {
        let current = self.read_register(offset::STREAM_CHANNEL_0_PACKET_SIZE)?;
        let new_value = (current & 0xffff_0000) | size as u32;
        self.write_register(offset::STREAM_CHANNEL_0_PACKET_SIZE, new_value)
    }

    /// Read a feature by name (`device.read::<f64>("ExposureTime")`).
    pub fn read<T: FeatureValue>(&self, name: &str) -> Result<T> {
        T::get(self, name)
    }

    /// Write a feature by name (`device.write("ExposureTime", 1000.0)`).
    pub fn write<T: FeatureValue>(&self, name: &str, value: T) -> Result<()> {
        T::set(self, name, value)
    }

    /// Execute a GenICam `Command` feature (e.g. `"AcquisitionStart"`).
    pub fn execute_command(&self, name: &str) -> Result<()> {
        self.with_io(|tree, io| tree.execute_command(io, name).map_err(|e| Error::GenIcam(e.to_string())))
    }

    /// This device's GenICam XML, exactly as it was fetched at connect time.
    pub fn genicam_xml(&self) -> &str {
        &self.genicam_xml
    }

    /// The GenICam node kind for `name` (`"Integer"`, `"Float"`, `"Enumeration"`, `"StringReg"`,
    /// `"Command"`, …), or `None` when this device's XML has no such node or it is a kind this
    /// crate does not model.
    ///
    /// Useful for deciding which typed accessor to call before calling it, and for telling
    /// apart "this camera has no such feature" from "this feature exists but cannot be
    /// evaluated" — a distinction [`Device::read`] flattens into an error string.
    pub fn feature_kind(&self, name: &str) -> Option<&'static str> {
        self.genicam.node_id(name).ok().map(|id| self.genicam.node_kind(id))
    }

    /// The feature names directly under the `"Root"` category, in XML document order.
    pub fn categories(&self) -> Result<Vec<String>> {
        self.genicam
            .category_children("Root")
            .map_err(|e| Error::GenIcam(e.to_string()))
    }

    pub fn read_register(&self, address: u32) -> Result<u32> {
        self.conn.lock().unwrap().read_register(address)
    }

    pub fn write_register(&self, address: u32, value: u32) -> Result<()> {
        self.conn.lock().unwrap().write_register(address, value)
    }

    pub fn read_memory(&self, address: u32, len: usize) -> Result<Vec<u8>> {
        self.conn.lock().unwrap().read_memory(address, len)
    }

    pub fn write_memory(&self, address: u32, data: &[u8]) -> Result<()> {
        self.conn.lock().unwrap().write_memory(address, data)
    }

    /// `false` once the heartbeat thread has observed a lost/never-acquired control channel.
    pub fn has_control(&self) -> bool {
        !self.control_lost.load(Ordering::Relaxed)
    }

    /// Stop the heartbeat thread without releasing control-channel privilege on the device
    /// (useful for tests that want to simulate an ungraceful disconnect).
    pub fn stop_heartbeat(&mut self) {
        if let Some(mut h) = self.heartbeat.take() {
            h.stop();
        }
    }

    /// Release control-channel privilege and stop the heartbeat thread.
    pub fn shutdown(&mut self) {
        self.stop_heartbeat();
        let _ = self
            .conn
            .lock()
            .unwrap()
            .write_register(offset::CONTROL_CHANNEL_PRIVILEGE, 0);
    }

    /// A minimal, genuinely `Send + Sync` handle for issuing `PACKET_RESEND_CMD`s from another
    /// thread (e.g. a GVSP receiver thread). `Device` itself isn't `Sync` (its GenICam feature
    /// cache uses plain `Cell`/`RefCell`, since register access is otherwise always funneled
    /// through the single `Mutex`-guarded transaction) — this hands out just the piece that
    /// needs cross-thread access.
    pub fn resend_sender(&self) -> PacketResendSender {
        PacketResendSender(self.conn.clone())
    }
}

/// See [`Device::resend_sender`].
pub struct PacketResendSender(Arc<Mutex<GvcpTransaction>>);

impl PacketResendSender {
    pub fn send(&self, resend: aravis_port_core::gvcp::PacketResend) -> Result<()> {
        self.0.lock().unwrap().send_packet_resend(resend)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shutdown();
    }
}

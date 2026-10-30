#![forbid(unsafe_code)]
//! An in-process GVCP/GVSP simulator for `aravis-port` tests. Binds a UDP socket for GVCP
//! (discovery/register/memory commands, serving a hand-authored GenICam XML document; loopback
//! on an ephemeral port by default, see [`FakeCameraConfig::gvcp_bind`]) and a second one for
//! GVSP (frame-paced streaming once a client sets up a stream channel and starts acquisition).
//! Frames come from a [`FrameSource`], the built-in test patterns ([`PatternSource`]) unless
//! the caller sets another.

mod gvcp_server;
mod gvsp_server;
mod loss;
mod pattern;
mod registers;
mod source;
mod xml;

use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aravis_port_core::MacAddress;

use gvcp_server::SharedState;
use gvsp_server::GvspSettings;
use registers::{Identity, RegisterBank};
use source::SharedSource;

pub use gvsp_server::{CHUNK_ID_FRAME_ID, CHUNK_ID_IMAGE, CHUNK_MODE_EXTRA_BYTES};
pub use registers::{feature, pixel_format, scan_type, REGISTER_SPACE_SIZE};
pub use source::{FrameRequest, FrameSource, PatternSource};

/// Configuration for a simulated camera.
#[derive(Debug, Clone)]
pub struct FakeCameraConfig {
    pub manufacturer: String,
    pub model: String,
    pub version: String,
    pub serial: String,
    pub mac: MacAddress,
    pub heartbeat_timeout: Duration,
    /// The sensor size, which `WidthMax`/`HeightMax` report and the region (`Width`/`Height`
    /// plus `OffsetX`/`OffsetY`) must fit into. 4096 × 4096 by default.
    pub sensor_width: u32,
    pub sensor_height: u32,
    /// Where GVCP is served: `127.0.0.1:0` (an ephemeral loopback port) by default, or for
    /// example `10.0.0.5:3956` to act as a device on a real interface. The bound IP is what the
    /// bootstrap `CurrentIPAddress` and the discovery ack report. [`FakeCamera::start_on`]
    /// takes an already bound socket instead.
    pub gvcp_bind: SocketAddrV4,
    /// The IP stream, resend and test packets are sent from. `None` uses the GVCP socket's IP,
    /// so packets come from the camera's own address.
    pub gvsp_source_ip: Option<Ipv4Addr>,
    /// Answer discovery only from requesters in the camera's subnet under this mask (so a
    /// camera on `10.0.0.5` with `255.255.255.0` stays silent to `192.168.1.10`). `None`, the
    /// default, answers every requester.
    pub discovery_netmask: Option<Ipv4Addr>,
    /// How often a new frame is sent while acquisition is active, unless paced from the frame
    /// rate (see [`Self::pace_from_frame_rate`]).
    pub frame_period: Duration,
    /// Pace frames from `AcquisitionFrameRate` while `AcquisitionFrameRateEnable` is set, as a
    /// free-running camera does. Off by default, so tests keep [`Self::frame_period`] (the
    /// power-on enable is set, at 30 Hz).
    pub pace_from_frame_rate: bool,
    /// How many of the last sent frames are kept to answer resends. A resend of an older frame
    /// is answered with "packet unavailable" error packets.
    pub resend_history: usize,
    /// Power-on `GevSCPSPacketSize`, and so the GVSP packet size used to chunk each frame's
    /// payload until a client programs another.
    pub packet_size: u16,
    /// The largest packet that gets from this camera to the client: stream and test packets
    /// (`GevSCPSFireTestPacket`) bigger than this are lost, as on a link with that MTU.
    pub path_mtu: u16,
    /// Whether test packets are sent at all. `false` models a device without support for them.
    pub test_packets: bool,
    /// Answer every packet-resend request with "packet unavailable" error packets (no data)
    /// instead of the packets, like a device whose send buffer already dropped them.
    pub resend_unavailable: bool,
    /// Independent per-packet drop probability (0.0-1.0), applied to each leader/payload/trailer
    /// packet, to drive packet-resend tests.
    pub gvsp_loss_probability: f64,
}

impl Default for FakeCameraConfig {
    fn default() -> Self {
        Self {
            manufacturer: "aravis-port".to_string(),
            model: "FakeCamera".to_string(),
            version: "0.1".to_string(),
            serial: "0001".to_string(),
            mac: MacAddress::new([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]),
            heartbeat_timeout: Duration::from_millis(3000),
            sensor_width: 4096,
            sensor_height: 4096,
            gvcp_bind: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0),
            gvsp_source_ip: None,
            discovery_netmask: None,
            frame_period: Duration::from_millis(40),
            pace_from_frame_rate: false,
            resend_history: 16,
            packet_size: 1500,
            path_mtu: 9000,
            test_packets: true,
            resend_unavailable: false,
            gvsp_loss_probability: 0.0,
        }
    }
}

struct ServerThread {
    stop_tx: Sender<()>,
    join: JoinHandle<()>,
}

/// A running in-process fake camera. GVCP requests are served at [`FakeCamera::local_addr`]
/// (an ephemeral loopback port by default); dropping this value stops both server threads.
pub struct FakeCamera {
    shared: Arc<Mutex<SharedState>>,
    source: SharedSource,
    local_addr: SocketAddrV4,
    gvcp: Option<ServerThread>,
    gvsp: Option<ServerThread>,
}

impl FakeCamera {
    /// Bind GVCP at [`FakeCameraConfig::gvcp_bind`] and start the camera.
    pub fn start(cfg: FakeCameraConfig) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(cfg.gvcp_bind)?;
        Self::start_on(cfg, socket)
    }

    /// Start the camera on an already bound GVCP socket, for sockets with options std can't
    /// set (`SO_REUSEADDR`, broadcast). [`FakeCameraConfig::gvcp_bind`] is ignored. The socket
    /// must be bound to an IPv4 address.
    pub fn start_on(cfg: FakeCameraConfig, socket: UdpSocket) -> std::io::Result<Self> {
        let local_addr = match socket.local_addr()? {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "the GVCP socket must be bound to an IPv4 address",
                ))
            }
        };
        let stream_source_ip = cfg.gvsp_source_ip.unwrap_or(*local_addr.ip());

        let identity = Identity {
            manufacturer: cfg.manufacturer.clone(),
            model: cfg.model.clone(),
            version: cfg.version.clone(),
            serial: cfg.serial.clone(),
            mac: cfg.mac,
            current_ip: *local_addr.ip(),
        };
        let mut bank = RegisterBank::new(
            &identity,
            xml::minimal_genicam_xml(cfg.sensor_width, cfg.sensor_height),
        );
        bank.write_u32(
            aravis_port_core::bootstrap::offset::HEARTBEAT_TIMEOUT,
            cfg.heartbeat_timeout.as_millis() as u32,
        );
        bank.write_u32(
            aravis_port_core::bootstrap::offset::STREAM_CHANNEL_0_PACKET_SIZE,
            u32::from(cfg.packet_size),
        );

        let shared = Arc::new(Mutex::new(SharedState {
            bank,
            controller: None,
            heartbeat_deadline: Instant::now(),
            packet_size: cfg.packet_size,
            path_mtu: cfg.path_mtu,
            test_packets: cfg.test_packets,
            resend_unavailable: cfg.resend_unavailable,
            discovery_netmask: cfg.discovery_netmask,
            stream_source_ip,
            history: VecDeque::new(),
            history_len: cfg.resend_history,
        }));
        let source: SharedSource = Arc::new(Mutex::new(Box::new(PatternSource)));

        // GVSP first: a source IP that can't be bound fails here without a GVCP thread to stop.
        let (gvsp_stop_tx, gvsp_join) = gvsp_server::spawn(
            shared.clone(),
            source.clone(),
            GvspSettings {
                source_ip: stream_source_ip,
                frame_period: cfg.frame_period,
                pace_from_frame_rate: cfg.pace_from_frame_rate,
                packet_size: cfg.packet_size,
                loss_probability: cfg.gvsp_loss_probability,
            },
        )?;
        let (gvcp_stop_tx, gvcp_join) =
            gvcp_server::spawn(socket, shared.clone(), cfg.heartbeat_timeout);

        Ok(Self {
            shared,
            source,
            local_addr,
            gvcp: Some(ServerThread {
                stop_tx: gvcp_stop_tx,
                join: gvcp_join,
            }),
            gvsp: Some(ServerThread {
                stop_tx: gvsp_stop_tx,
                join: gvsp_join,
            }),
        })
    }

    /// The address/port this camera's GVCP server is listening on.
    pub fn local_addr(&self) -> SocketAddrV4 {
        self.local_addr
    }

    /// Replace the frame source, also while streaming: the next frame comes from `source`.
    /// Frames sent before keep their bytes for resends.
    pub fn set_frame_source(&self, source: impl FrameSource + 'static) {
        *self.source.lock().unwrap_or_else(|e| e.into_inner()) = Box::new(source);
    }

    /// Read a register directly (bypassing GVCP), for test assertions.
    pub fn peek_register(&self, address: u32) -> u32 {
        self.shared.lock().unwrap().bank.read_u32(address)
    }

    /// Write a register directly (bypassing GVCP and access control), for test setup.
    pub fn poke_register(&self, address: u32, value: u32) {
        self.shared.lock().unwrap().bank.write_u32(address, value);
    }

    /// The address of the client currently holding control-channel privilege, if any.
    pub fn controller(&self) -> Option<SocketAddr> {
        self.shared.lock().unwrap().controller
    }

    /// The raw GenICam XML this camera serves, for test assertions against fetched copies.
    pub fn xml_bytes(&self) -> Vec<u8> {
        self.shared.lock().unwrap().bank.xml.clone()
    }
}

impl Drop for FakeCamera {
    fn drop(&mut self) {
        for thread in [self.gvcp.take(), self.gvsp.take()].into_iter().flatten() {
            let _ = thread.stop_tx.send(());
            let _ = thread.join.join();
        }
    }
}

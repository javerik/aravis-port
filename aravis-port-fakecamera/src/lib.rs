#![forbid(unsafe_code)]
//! An in-process GVCP/GVSP simulator for `aravis-port` tests. Binds a loopback UDP socket for
//! GVCP (discovery/register/memory commands, serving a hand-authored GenICam XML document) and
//! a second one for GVSP (frame-paced test-pattern streaming once a client sets up a stream
//! channel and starts acquisition). Packet-loss simulation is added in a later phase.

mod gvcp_server;
mod gvsp_server;
mod loss;
mod pattern;
mod registers;
mod xml;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aravis_port_core::MacAddress;

use gvcp_server::SharedState;
use registers::{Identity, RegisterBank};

pub use gvsp_server::CHUNK_ID_FRAME_ID;
pub use registers::{feature, pixel_format, REGISTER_SPACE_SIZE};

/// Configuration for a simulated camera.
#[derive(Debug, Clone)]
pub struct FakeCameraConfig {
    pub manufacturer: String,
    pub model: String,
    pub version: String,
    pub serial: String,
    pub mac: MacAddress,
    pub heartbeat_timeout: Duration,
    /// How often a new frame is sent while acquisition is active.
    pub frame_period: Duration,
    /// GVSP packet size used to chunk each frame's payload.
    pub packet_size: u16,
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
            frame_period: Duration::from_millis(40),
            packet_size: 1500,
            gvsp_loss_probability: 0.0,
        }
    }
}

struct ServerThread {
    stop_tx: Sender<()>,
    join: JoinHandle<()>,
}

/// A running in-process fake camera. GVCP requests are served on an ephemeral loopback port
/// (see [`FakeCamera::local_addr`]); dropping this value stops both server threads.
pub struct FakeCamera {
    shared: Arc<Mutex<SharedState>>,
    local_addr: SocketAddrV4,
    gvcp: Option<ServerThread>,
    gvsp: Option<ServerThread>,
}

impl FakeCamera {
    pub fn start(cfg: FakeCameraConfig) -> std::io::Result<Self> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let local_addr = match socket.local_addr()? {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => unreachable!("bound to an IPv4 loopback address"),
        };

        let identity = Identity {
            manufacturer: cfg.manufacturer.clone(),
            model: cfg.model.clone(),
            version: cfg.version.clone(),
            serial: cfg.serial.clone(),
            mac: cfg.mac,
            current_ip: *local_addr.ip(),
        };
        let mut bank = RegisterBank::new(&identity, xml::minimal_genicam_xml());
        bank.write_u32(
            aravis_port_core::bootstrap::offset::HEARTBEAT_TIMEOUT,
            cfg.heartbeat_timeout.as_millis() as u32,
        );

        let shared = Arc::new(Mutex::new(SharedState {
            bank,
            controller: None,
            heartbeat_deadline: Instant::now(),
            packet_size: cfg.packet_size,
        }));

        let (gvcp_stop_tx, gvcp_join) = gvcp_server::spawn(socket, shared.clone(), cfg.heartbeat_timeout);
        let (gvsp_stop_tx, gvsp_join) =
            gvsp_server::spawn(shared.clone(), cfg.frame_period, cfg.packet_size, cfg.gvsp_loss_probability)?;

        Ok(Self {
            shared,
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

    /// The loopback address/port this camera's GVCP server is listening on.
    pub fn local_addr(&self) -> SocketAddrV4 {
        self.local_addr
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

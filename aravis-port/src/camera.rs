use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use aravis_port_core::gvcp::PacketResend;
use aravis_port_core::memory::{new_buffer_pool, Buffer, BufferPoolHandle};
use aravis_port_core::{Error, Result};
use aravis_port_device::net::DiscoveredDevice;
use aravis_port_device::{Device, DeviceConfig, FeatureValue, PacketResendSender};
use aravis_port_stream::{ResendRequester, StreamConfig};

/// A safely-under-standard-MTU default GVSP packet size — a device's power-on default can
/// exceed 1500 once IP/UDP headers are added (confirmed on the live C5-2040-GigE camera, whose
/// default of 1501 does exactly this), so `start_stream` sets this before acquiring.
const DEFAULT_STREAM_PACKET_SIZE: u16 = 1400;

struct DeviceResendRequester {
    sender: PacketResendSender,
}

impl ResendRequester for DeviceResendRequester {
    fn request_resend(&mut self, resend: PacketResend) {
        let _ = self.sender.send(resend);
    }
}

/// A connected camera: the flat, convenience API over `aravis-port-device`/`-stream`.
pub struct Camera {
    device: Device,
    ip: Ipv4Addr,
}

impl Camera {
    /// Connect to a camera found by [`crate::discover`].
    pub fn new(info: &DiscoveredDevice) -> Result<Self> {
        Self::connect(info.current_ip)
    }

    /// Connect directly by IP address (standard GVCP port 3956), without a prior discovery round.
    pub fn connect(ip: Ipv4Addr) -> Result<Self> {
        Self::connect_addr(SocketAddrV4::new(ip, aravis_port_core::gvcp::PORT))
    }

    /// Connect to a specific address/port. Real cameras always listen on the standard GVCP port
    /// (3956, see [`Camera::connect`]); this exists for non-standard setups (e.g. a simulated
    /// device bound to an ephemeral port in tests).
    pub fn connect_addr(addr: SocketAddrV4) -> Result<Self> {
        let device = Device::connect(addr, DeviceConfig::default())?;
        Ok(Self {
            device,
            ip: *addr.ip(),
        })
    }

    /// Read a feature by name (`camera.read::<f64>("ExposureTime")`).
    pub fn read<T: FeatureValue>(&self, name: &str) -> Result<T> {
        self.device.read(name)
    }

    /// Write a feature by name (`camera.write("ExposureTime", 1000.0)`).
    pub fn write<T: FeatureValue>(&self, name: &str, value: T) -> Result<()> {
        self.device.write(name, value)
    }

    /// Read a chunk feature (e.g. `"ChunkFrameID"`) from a buffer acquired with chunk mode
    /// active. See [`Device::read_chunk`].
    pub fn read_chunk<T: FeatureValue>(&self, buffer: &Buffer, name: &str) -> Result<T> {
        self.device.read_chunk(buffer, name)
    }

    /// Whether feature `name` is currently available. See [`Device::is_available`].
    pub fn is_available(&self, name: &str) -> Result<bool> {
        self.device.is_available(name)
    }

    /// Execute a GenICam `Command` feature (e.g. `"AcquisitionStart"`).
    pub fn execute_command(&self, name: &str) -> Result<()> {
        self.device.execute_command(name)
    }

    /// The feature names directly under the `"Root"` category.
    pub fn categories(&self) -> Result<Vec<String>> {
        self.device.categories()
    }

    /// This camera's GenICam XML, exactly as it was fetched at connect time.
    pub fn genicam_xml(&self) -> &str {
        self.device.genicam_xml()
    }

    /// The GenICam node kind for `name` (`"Integer"`, `"Enumeration"`, `"StringReg"`, …), or
    /// `None` when this camera's XML has no such node or it is a kind this crate does not model.
    /// See [`Device::feature_kind`].
    pub fn feature_kind(&self, name: &str) -> Option<&'static str> {
        self.device.feature_kind(name)
    }

    /// `false` once the heartbeat thread has observed lost control-channel privilege.
    pub fn has_control(&self) -> bool {
        self.device.has_control()
    }

    /// Direct access to the underlying `Device` — for raw register/memory I/O
    /// (`read_register`/`write_register`/`read_memory`/`write_memory`) and other lower-level
    /// operations `Camera` doesn't wrap directly.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Mutable access to the underlying `Device` — needed for `stop_heartbeat`/`shutdown`, which
    /// take `&mut self` on `Device` (e.g. to simulate an ungraceful disconnect in tests).
    pub fn device_mut(&mut self) -> &mut Device {
        &mut self.device
    }

    /// Figure out which local IP address the OS would route through to reach `peer`, without
    /// sending any packets — `std` has no direct "outgoing interface for this destination" query,
    /// but `UdpSocket::connect` resolves routing and `local_addr()` then reports it.
    fn local_route_to(peer_ip: Ipv4Addr) -> Result<Ipv4Addr> {
        let probe = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).map_err(Error::Io)?;
        probe.connect((peer_ip, aravis_port_core::gvcp::PORT)).map_err(Error::Io)?;
        match probe.local_addr().map_err(Error::Io)?.ip() {
            std::net::IpAddr::V4(v4) => Ok(v4),
            std::net::IpAddr::V6(_) => Err(Error::GenIcam("expected an IPv4 route to the camera".to_string())),
        }
    }

    /// Set up a GVSP stream channel and buffer pool; returns the low-level pool handle plus the
    /// running receiver. Shared by every `start_stream*` variant. `cfg.packet_size` is written
    /// to the device (`GevSCPSPacketSize`) before acquiring, so the two always stay in sync —
    /// setting the packet size any other way (e.g. `camera.write("GevSCPSPacketSize", ...)`)
    /// before calling a `start_stream*` method has no effect, since this overwrites it.
    fn open_stream(&self, cfg: StreamConfig) -> Result<(BufferPoolHandle, aravis_port_stream::StreamHandle)> {
        let local_ip = Self::local_route_to(self.ip)?;
        let socket = UdpSocket::bind((local_ip, 0)).map_err(Error::Io)?;
        let local_port = socket.local_addr().map_err(Error::Io)?.port();

        self.device.open_stream_channel(local_ip, local_port)?;
        self.device.set_stream_packet_size(cfg.packet_size)?;
        // Only now, with the packet size programmed: some devices' PayloadSize depends on it
        // (confirmed on the live C6-2040-GigE: 409848 bytes at 1000, 410072 at 1400), and a
        // pool sized for another packet size is too small for every frame.
        let payload_size: i64 = self.device.read("PayloadSize")?;

        let (pool_user, pool_stream) = new_buffer_pool(4, payload_size.max(0) as usize);
        let requester = Box::new(DeviceResendRequester {
            sender: self.device.resend_sender(),
        });
        let stream_handle = aravis_port_stream::spawn(socket, cfg, pool_stream, requester, None).map_err(Error::Io)?;

        // Best-effort: most GEV cameras expose a standard "AcquisitionStart" command, and the
        // convenience API should "just work" without the caller needing to know that name. Not
        // fatal if it's missing/fails — some devices free-run once the stream channel is set up,
        // or use a different mechanism this crate doesn't yet model.
        let _ = self.device.execute_command("AcquisitionStart");

        Ok((pool_user, stream_handle))
    }

    fn default_stream_config() -> StreamConfig {
        StreamConfig {
            packet_size: DEFAULT_STREAM_PACKET_SIZE,
            ..StreamConfig::default()
        }
    }

    /// Start streaming, invoking `callback` with a clone of each completed frame (the original
    /// buffer is recycled automatically), using a safely-under-MTU default packet size and the
    /// default resend/timeout tuning. For zero-copy access to buffers, use
    /// [`Camera::start_stream_channel`]; to control the packet size or resend/timeout behavior,
    /// use [`Camera::start_stream_with_config`].
    pub fn start_stream(&self, callback: impl FnMut(Buffer) + Send + 'static) -> Result<StreamHandle> {
        self.start_stream_with_config(Self::default_stream_config(), callback)
    }

    /// Like [`Camera::start_stream`], but with caller-supplied [`StreamConfig`] — most commonly
    /// to set a non-default `packet_size` (e.g. jumbo frames on a high-MTU network), but also to
    /// tune resend/timeout behavior. `cfg.packet_size` is written to the device as
    /// `GevSCPSPacketSize` before acquiring.
    pub fn start_stream_with_config(&self, cfg: StreamConfig, mut callback: impl FnMut(Buffer) + Send + 'static) -> Result<StreamHandle> {
        let (pool_user, stream_handle) = self.open_stream(cfg)?;
        let (stop_tx, stop_rx) = mpsc::channel();
        let join = thread::spawn(move || loop {
            if stop_rx.try_recv().is_ok() {
                return;
            }
            if let Some(buf) = pool_user.timeout_pop_buffer(Duration::from_millis(100)) {
                callback(buf.clone());
                pool_user.push_buffer(buf);
            }
        });
        Ok(StreamHandle {
            _stream: stream_handle,
            delivery_stop: Some(stop_tx),
            delivery_join: Some(join),
        })
    }

    /// Start streaming with direct access to the buffer pool: pop completed buffers from the
    /// returned [`BufferPoolHandle`] and push them back once done, for zero-copy reuse. Uses the
    /// same default packet size as [`Camera::start_stream`]; see
    /// [`Camera::start_stream_channel_with_config`] to override it.
    pub fn start_stream_channel(&self) -> Result<(StreamHandle, BufferPoolHandle)> {
        self.start_stream_channel_with_config(Self::default_stream_config())
    }

    /// Like [`Camera::start_stream_channel`], but with caller-supplied [`StreamConfig`] — see
    /// [`Camera::start_stream_with_config`].
    pub fn start_stream_channel_with_config(&self, cfg: StreamConfig) -> Result<(StreamHandle, BufferPoolHandle)> {
        let (pool_user, stream_handle) = self.open_stream(cfg)?;
        Ok((
            StreamHandle {
                _stream: stream_handle,
                delivery_stop: None,
                delivery_join: None,
            },
            pool_user,
        ))
    }

    /// Stop a stream started by any `start_stream*` method.
    pub fn stop_stream(&self, handle: StreamHandle) -> Result<()> {
        drop(handle);
        let _ = self.device.execute_command("AcquisitionStop");
        Ok(())
    }
}

/// Handle to a running stream; dropping it (or passing it to [`Camera::stop_stream`]) stops the
/// receiver thread (and, for [`Camera::start_stream`]/[`Camera::start_stream_with_config`], the
/// delivery thread).
pub struct StreamHandle {
    _stream: aravis_port_stream::StreamHandle,
    delivery_stop: Option<mpsc::Sender<()>>,
    delivery_join: Option<JoinHandle<()>>,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.delivery_stop.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.delivery_join.take() {
            let _ = join.join();
        }
        // `_stream` (aravis_port_stream::StreamHandle) stops itself in its own `Drop`.
    }
}

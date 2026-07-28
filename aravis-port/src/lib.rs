#![forbid(unsafe_code)]
//! `aravis-port`: a pure-Rust, zero-`unsafe` GigE Vision (GEV) camera control and streaming
//! library. Users should generally only need this crate — lower-level access is available via
//! the re-exported sub-crate modules (`core`, `net`, `genicam`, `memory`, `device`, `stream`).
//!
//! ```no_run
//! use aravis_port::prelude::*;
//! use std::time::Duration;
//!
//! # fn main() -> Result<()> {
//! let cameras = aravis_port::discover(Duration::from_secs(2))?;
//! let camera = Camera::new(&cameras[0])?;
//!
//! let exposure = camera.read::<f64>("ExposureTime")?;
//! println!("ExposureTime = {exposure}");
//!
//! let stream = camera.start_stream(|buffer| {
//!     println!("Received frame id {} size {}", buffer.frame_id, buffer.data().len());
//! })?;
//!
//! std::thread::sleep(Duration::from_secs(5));
//! camera.stop_stream(stream)?;
//! # Ok(())
//! # }
//! ```

mod camera;

use std::time::Duration;

pub use aravis_port_core as core;
pub use aravis_port_device as device;
pub use aravis_port_genicam as genicam;
pub use aravis_port_memory as memory;
pub use aravis_port_net as net;
pub use aravis_port_stream as stream;

pub use aravis_port_core::{Error, Result};
pub use aravis_port_net::DiscoveredDevice;
pub use aravis_port_stream::StreamConfig;

pub use camera::{Camera, StreamHandle};

/// Discover cameras on the network within `timeout`. Blocks for the full duration of the
/// broadcast round. See [`aravis_port_net::DiscoveryOptions`] for interface-selection control
/// beyond the default (`0.0.0.0`, i.e. the OS default route) — `std` has no safe API to
/// enumerate local network interfaces automatically.
pub fn discover(timeout: Duration) -> Result<Vec<DiscoveredDevice>> {
    aravis_port_net::discover(&aravis_port_net::DiscoveryOptions {
        timeout,
        ..Default::default()
    })
}

/// Re-exports covering the common case: `use aravis_port::prelude::*;`.
pub mod prelude {
    pub use crate::{discover, Camera, Error, Result, StreamConfig, StreamHandle};
    pub use aravis_port_memory::Buffer;
}

#![forbid(unsafe_code)]
//! GVCP device client, GenICam feature bridge, and heartbeat management (Phase 2/3).

mod device;
mod feature;
mod heartbeat;
mod io;
mod xml_fetch;
mod zip;

pub use device::{Device, DeviceConfig, PacketResendSender};
pub use feature::FeatureValue;

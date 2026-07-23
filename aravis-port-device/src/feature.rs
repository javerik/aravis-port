use aravis_port_core::{Error, Result};

use crate::device::Device;

/// Bridges GenICam feature access to Rust primitives, so callers can write
/// `device.read::<f64>("ExposureTime")` / `device.write("Gain", 12i64)` without naming the
/// underlying GenICam node kind.
pub trait FeatureValue: Sized {
    fn get(device: &Device, name: &str) -> Result<Self>;
    fn set(device: &Device, name: &str, value: Self) -> Result<()>;
}

fn wrap<T>(result: aravis_port_genicam::Result<T>) -> Result<T> {
    result.map_err(|e| Error::GenIcam(e.to_string()))
}

impl FeatureValue for i64 {
    fn get(device: &Device, name: &str) -> Result<Self> {
        device.with_io(|tree, io| wrap(tree.get_integer(io, name)))
    }
    fn set(device: &Device, name: &str, value: Self) -> Result<()> {
        device.with_io(|tree, io| wrap(tree.set_integer(io, name, value)))
    }
}

impl FeatureValue for f64 {
    fn get(device: &Device, name: &str) -> Result<Self> {
        device.with_io(|tree, io| wrap(tree.get_float(io, name)))
    }
    fn set(device: &Device, name: &str, value: Self) -> Result<()> {
        device.with_io(|tree, io| wrap(tree.set_float(io, name, value)))
    }
}

impl FeatureValue for bool {
    fn get(device: &Device, name: &str) -> Result<Self> {
        device.with_io(|tree, io| wrap(tree.get_boolean(io, name)))
    }
    fn set(device: &Device, name: &str, value: Self) -> Result<()> {
        device.with_io(|tree, io| wrap(tree.set_boolean(io, name, value)))
    }
}

/// Reads/writes an `Enumeration` by its symbolic entry name, or a `StringReg` by its raw text.
impl FeatureValue for String {
    fn get(device: &Device, name: &str) -> Result<Self> {
        device.with_io(|tree, io| {
            let id = wrap(tree.node_id(name))?;
            match tree.node_kind(id) {
                "Enumeration" => wrap(tree.get_enum_symbolic(io, name)),
                _ => wrap(tree.get_string(io, name)),
            }
        })
    }
    fn set(device: &Device, name: &str, value: Self) -> Result<()> {
        device.with_io(|tree, io| {
            let id = wrap(tree.node_id(name))?;
            match tree.node_kind(id) {
                "Enumeration" => wrap(tree.set_enum_symbolic(io, name, &value)),
                _ => wrap(tree.set_string(io, name, &value)),
            }
        })
    }
}

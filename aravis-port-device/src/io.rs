use std::io;
use std::sync::{Arc, Mutex};

use aravis_port_genicam::{RegisterAccess, RegisterDescription};

use crate::net::GvcpTransaction;

/// Devices whose GenICam XML declares schema 1.1 or later but which still implement the legacy
/// (GenICam 1.0) register access mechanism, as `(VendorName, ModelName)` from the XML's
/// `<RegisterDescription>` element. Copied from Aravis 0.9's `arv_gc_port_legacy_infos` (which
/// matches them as glob patterns; none of them contains a wildcard, so exact comparison is
/// equivalent). Confirmed on the live C6-2040-GigE: it answers READMEM on its 4-byte feature
/// registers with `GEV_STATUS_INVALID_ADDRESS` but serves the same addresses through READREG.
const LEGACY_REGISTER_ACCESS_DEVICES: &[(&str, &str)] = &[
    ("AT_Automation_Technology_GmbH", "C6_X_GigE"),
    ("DO3THINK", "MGV518"),
    ("EVK", "HELIOS"),
    ("IDS_Imaging_Development_Systems_GmbH", "GV_53FxLE_M"),
    ("IDS_Imaging_Development_Systems_GmbH", "GV_58CxLE_M"),
    ("IDS_Imaging_Development_Systems_GmbH", "GV_524xCP_NIR"),
    ("IDS_Imaging_Development_Systems_GmbH", "GV_599xCP_C"),
    ("Imperx", "IpxGEVCamera"),
    ("KowaOptronics", "SC130ET3"),
    ("NIT", "Tachyon16k"),
    ("NIT", "NSC1601GIGE"),
    ("PleoraTechnologiesInc", "iPORTCLGigE"),
    ("PleoraTechnologiesInc", "NTxGigE"),
    ("Sony", "XCG_CGSeries"),
    ("Sony", "XCG_CPSeries"),
    ("TeledyneDALSA", "ICE"),
    ("Xenics", "Wildcat"),
    ("FLIR", "XscSeries"),
];

/// Whether 4-byte register accesses must use READREG/WRITEREG instead of READMEM/WRITEMEM (the
/// GenICam 1.0 "legacy endianness mechanism", GenICam standard appendix 3): true for documents
/// older than schema 1.1.0 and for the known devices that misreport a newer schema. Mirrors
/// Aravis's `_use_legacy_endianness_mechanism`.
pub(crate) fn uses_legacy_register_access(description: &RegisterDescription) -> bool {
    description.schema_version < (1, 1, 0)
        || LEGACY_REGISTER_ACCESS_DEVICES
            .iter()
            .any(|&(vendor, model)| {
                description.vendor_name == vendor && description.model_name == model
            })
}

/// Bridges `aravis-port-genicam`'s transport-agnostic `RegisterAccess` trait to the shared,
/// mutex-guarded GVCP transaction (shared with the heartbeat thread).
pub(crate) struct GvcpTransactionIo {
    pub conn: Arc<Mutex<GvcpTransaction>>,
    /// See [`uses_legacy_register_access`]. In legacy mode registers are big-endian, so a
    /// READREG value maps to the same byte order READMEM would have returned.
    pub legacy_register_access: bool,
}

fn to_io_error(e: aravis_port_core::Error) -> io::Error {
    io::Error::other(e.to_string())
}

impl RegisterAccess for GvcpTransactionIo {
    fn read_memory(&mut self, address: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut conn = self.conn.lock().unwrap();
        if self.legacy_register_access && len == 4 {
            let value = conn.read_register(address as u32).map_err(to_io_error)?;
            return Ok(value.to_be_bytes().to_vec());
        }
        conn.read_memory(address as u32, len).map_err(to_io_error)
    }

    fn write_memory(&mut self, address: u64, data: &[u8]) -> io::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        if let (true, Ok(bytes)) = (self.legacy_register_access, <[u8; 4]>::try_from(data)) {
            return conn
                .write_register(address as u32, u32::from_be_bytes(bytes))
                .map_err(to_io_error);
        }
        conn.write_memory(address as u32, data).map_err(to_io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(
        vendor: &str,
        model: &str,
        schema_version: (u32, u32, u32),
    ) -> RegisterDescription {
        RegisterDescription {
            vendor_name: vendor.to_string(),
            model_name: model.to_string(),
            schema_version,
        }
    }

    #[test]
    fn legacy_register_access_follows_schema_version_and_quirk_list() {
        assert!(uses_legacy_register_access(&description(
            "Any",
            "Camera",
            (1, 0, 9)
        )));
        assert!(uses_legacy_register_access(&description(
            "Any",
            "Camera",
            (0, 0, 0)
        )));
        assert!(!uses_legacy_register_access(&description(
            "Any",
            "Camera",
            (1, 1, 0)
        )));
        assert!(!uses_legacy_register_access(&description(
            "Any",
            "Camera",
            (2, 0, 0)
        )));
        // The live C6-2040-GigE's XML declares schema 1.1.0 but needs legacy access.
        assert!(uses_legacy_register_access(&description(
            "AT_Automation_Technology_GmbH",
            "C6_X_GigE",
            (1, 1, 0)
        )));
    }
}

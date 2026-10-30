use std::io;

use aravis_port_core::bootstrap::offset;
use aravis_port_core::zip;
use aravis_port_genicam::{GenApiTree, NodeId};

use crate::gvsp_server::CHUNK_MODE_EXTRA_BYTES;
use crate::registers::{feature, pixel_format, scan_type, RegisterBank};

/// Default address in the fake camera's 32-bit register space where a vendor GenICam XML (or
/// `.zip`) is served when not overridden via [`VendorXml::with_address`].
pub const DEFAULT_VENDOR_XML_ADDRESS: u32 = 0x0100_0000;

/// A caller-supplied GenICam XML (raw `.xml` or `.zip`-compressed) to serve from a [` crate::FakeCamera`].
#[derive(Debug, Clone)]
pub struct VendorXml {
    /// Raw bytes served over GVCP `READMEM` (either UTF-8 XML or a single-file `.zip` archive).
    pub bytes: Vec<u8>,
    /// Filename advertised in the bootstrap `XML_URL_0` (`Local:<file_name>;<addr>;<len>`).
    /// Must end in `.zip` when `bytes` is a ZIP archive so clients know to decompress it.
    pub file_name: String,
    /// Start address in the 32-bit register space where `bytes` is mapped read-only.
    pub address: u32,
}

impl VendorXml {
    /// Wrap raw `.xml` or `.zip` bytes, serving them at [`DEFAULT_VENDOR_XML_ADDRESS`].
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();
        let file_name = if zip::is_zip(&bytes) {
            "genicam.xml.zip".to_string()
        } else {
            "genicam.xml".to_string()
        };
        Self {
            bytes,
            file_name,
            address: DEFAULT_VENDOR_XML_ADDRESS,
        }
    }

    /// Set the filename advertised in `XML_URL_0`. If `bytes` is a ZIP archive, `.zip` is
    /// appended if not already present so GenICam clients decompress the payload.
    pub fn with_file_name(mut self, file_name: impl Into<String>) -> Self {
        let mut name = file_name.into();
        if zip::is_zip(&self.bytes) && !name.to_ascii_lowercase().ends_with(".zip") {
            name.push_str(".zip");
        }
        self.file_name = name;
        self
    }

    /// Set the 32-bit register address where the XML blob is served.
    pub fn with_address(mut self, address: u32) -> Self {
        self.address = address;
        self
    }

    /// Decompress (if `.zip`) and decode the XML text.
    pub fn xml_text(&self) -> io::Result<String> {
        let raw =
            if zip::is_zip(&self.bytes) || self.file_name.to_ascii_lowercase().ends_with(".zip") {
                zip::extract_first_file(&self.bytes).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("failed to extract zipped GenICam XML: {e}"),
                    )
                })?
            } else {
                self.bytes.clone()
            };
        String::from_utf8(raw).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("GenICam XML is not valid UTF-8: {e}"),
            )
        })
    }
}

/// Fallback sensor dimensions used when a vendor XML does not define `SensorWidth`/`SensorHeight`
/// or `WidthMax`/`HeightMax`, or before the XML's own `<Value>` defaults are applied.
#[derive(Debug, Clone, Copy)]
pub struct VendorDefaults {
    pub sensor_width: u32,
    pub sensor_height: u32,
}

/// Current SFNC feature values resolved by the emulator (either from the built-in register block
/// or through a vendor [`GenApiTree`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedFeatures {
    pub width: u32,
    pub height: u32,
    pub offset_x: u32,
    pub offset_y: u32,
    pub width_max: u32,
    pub height_max: u32,
    pub sensor_width: u32,
    pub sensor_height: u32,
    pub pixel_format: u32,
    pub pixel_format_name: String,
    pub payload_size: u32,
    pub acquisition_active: bool,
    pub frame_rate_enable: bool,
    pub frame_rate_hz: f32,
    pub tl_params_locked: bool,
    pub device_scan_type: u32,
    pub device_scan_type_name: String,
    pub packet_size: u32,
    pub chunk_mode_active: bool,
    pub exposure_time_us: u32,
    pub gain_raw: u32,
    pub reverse_x: bool,
    pub scan3d_coordinate_selector: u32,
}

impl ResolvedFeatures {
    /// Snapshot the built-in register bank into [`ResolvedFeatures`].
    pub fn from_builtin_bank(bank: &RegisterBank, sensor_width: u32, sensor_height: u32) -> Self {
        let width = bank.read_u32(feature::WIDTH).max(1);
        let height = bank.read_u32(feature::HEIGHT).max(1);
        let offset_x = bank.read_u32(feature::OFFSET_X);
        let offset_y = bank.read_u32(feature::OFFSET_Y);
        let pf = bank.read_u32(feature::PIXEL_FORMAT);
        let pf_name = pixel_format::name(pf).unwrap_or("Mono8").to_string();
        let bpp = bytes_per_pixel_for(&pf_name, pf);
        let chunk_mode_active = bank.read_u32(feature::CHUNK_MODE_ACTIVE) != 0;
        let payload_size = width
            .saturating_mul(height)
            .saturating_mul(bpp)
            .saturating_add(if chunk_mode_active {
                CHUNK_MODE_EXTRA_BYTES
            } else {
                0
            });
        let acquisition_active = bank.read_u32(feature::ACQUISITION_ACTIVE) != 0;
        let frame_rate_enable = bank.read_u32(feature::FRAME_RATE_ENABLE) != 0;
        let frame_rate_hz = f32::from_bits(bank.read_u32(feature::FRAME_RATE));
        let device_scan_type = bank.read_u32(feature::DEVICE_SCAN_TYPE);
        let device_scan_type_name = if device_scan_type == scan_type::LINESCAN3D {
            "Linescan3D".to_string()
        } else {
            "Areascan".to_string()
        };
        let packet_size = bank.read_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE) & 0xffff;
        Self {
            width,
            height,
            offset_x,
            offset_y,
            width_max: sensor_width,
            height_max: sensor_height,
            sensor_width,
            sensor_height,
            pixel_format: pf,
            pixel_format_name: pf_name,
            payload_size,
            acquisition_active,
            frame_rate_enable,
            frame_rate_hz,
            tl_params_locked: acquisition_active,
            device_scan_type,
            device_scan_type_name,
            packet_size,
            chunk_mode_active,
            exposure_time_us: bank.read_u32(feature::EXPOSURE_TIME_US),
            gain_raw: bank.read_u32(feature::GAIN_RAW),
            reverse_x: bank.read_u32(feature::REVERSE_X) != 0,
            scan3d_coordinate_selector: bank.read_u32(feature::SCAN3D_COORDINATE_SELECTOR),
        }
    }
}

/// Bytes per pixel for a symbolic PFNC/GenICam `PixelFormat` name, falling back to the PFNC
/// bit-depth field (`bits 16..24`) or the built-in fake camera's enum values.
pub fn bytes_per_pixel_for(symbolic: &str, reg_value: u32) -> u32 {
    match symbolic {
        "Mono8" | "BayerGR8" | "BayerRG8" | "BayerGB8" | "BayerBG8" | "Coord3D_C8" => 1,
        "Mono10" | "Mono12" | "Mono14" | "Mono16" | "Coord3D_C16" | "BayerGR10" | "BayerRG10"
        | "BayerGB10" | "BayerBG10" | "BayerGR12" | "BayerRG12" | "BayerGB12" | "BayerBG12"
        | "BayerGR16" | "BayerRG16" | "BayerGB16" | "BayerBG16" | "YUV422Packed"
        | "YUV422_YUYV_Packed" => 2,
        "RGB8" | "BGR8" | "RGB8Packed" | "BGR8Packed" | "Coord3D_ABC8" => 3,
        "RGBA8" | "BGRA8" | "RGBa8" | "BGRa8" | "Coord3D_AC16" => 4,
        "Coord3D_ABC16" => 6,
        _ => {
            let pfnc_bits = (reg_value >> 16) & 0xff;
            if pfnc_bits > 0 {
                pfnc_bits.div_ceil(8)
            } else {
                match reg_value {
                    pixel_format::MONO10 | pixel_format::MONO16 | pixel_format::COORD3D_C16 => 2,
                    _ => 1,
                }
            }
        }
    }
}

/// Parse and validate that `vendor` is well-formed GenICam XML and defines all required SFNC
/// features (`Width`, `Height`, `PixelFormat`, `AcquisitionStart`).
pub fn validate_vendor_xml(vendor: &VendorXml) -> io::Result<GenApiTree> {
    let text = vendor.xml_text()?;
    let tree = GenApiTree::parse(&text).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid GenICam XML: {e}"),
        )
    })?;
    validate_required_features(&tree)
        .map_err(|msg| io::Error::new(io::ErrorKind::InvalidData, msg))?;
    Ok(tree)
}

fn validate_required_features(tree: &GenApiTree) -> Result<(), String> {
    for numeric in ["Width", "Height"] {
        let id = tree
            .node_id(numeric)
            .map_err(|_| format!("GenICam XML is missing required SFNC feature '{numeric}'"))?;
        let kind = tree.node_kind(id);
        if !matches!(
            kind,
            "Integer" | "IntReg" | "SwissKnife" | "Converter" | "Float" | "FloatReg"
        ) {
            return Err(format!(
                "GenICam XML feature '{numeric}' must be numeric, found {kind}"
            ));
        }
    }

    let pf_id = tree
        .node_id("PixelFormat")
        .map_err(|_| "GenICam XML is missing required SFNC feature 'PixelFormat'".to_string())?;
    let pf_kind = tree.node_kind(pf_id);
    if pf_kind != "Enumeration" {
        return Err(format!(
            "GenICam XML feature 'PixelFormat' must be Enumeration, found {pf_kind}"
        ));
    }

    let acq_id = tree.node_id("AcquisitionStart").map_err(|_| {
        "GenICam XML is missing required SFNC feature 'AcquisitionStart'".to_string()
    })?;
    let acq_kind = tree.node_kind(acq_id);
    if acq_kind != "Command" {
        return Err(format!(
            "GenICam XML feature 'AcquisitionStart' must be Command, found {acq_kind}"
        ));
    }
    tree.command_spec("AcquisitionStart").map_err(|e| {
        format!("GenICam XML feature 'AcquisitionStart' has no writable target: {e}")
    })?;

    Ok(())
}

const SLOT_WIDTH: u32 = 1 << 0;
const SLOT_HEIGHT: u32 = 1 << 1;
const SLOT_OFFSET_X: u32 = 1 << 2;
const SLOT_OFFSET_Y: u32 = 1 << 3;
const SLOT_WIDTH_MAX: u32 = 1 << 4;
const SLOT_HEIGHT_MAX: u32 = 1 << 5;
const SLOT_SENSOR_WIDTH: u32 = 1 << 6;
const SLOT_SENSOR_HEIGHT: u32 = 1 << 7;
const SLOT_PIXEL_FORMAT: u32 = 1 << 8;
const SLOT_PAYLOAD_SIZE: u32 = 1 << 9;
const SLOT_ACQ_START: u32 = 1 << 10;
const SLOT_ACQ_STOP: u32 = 1 << 11;
const SLOT_FRAME_RATE: u32 = 1 << 12;
const SLOT_FRAME_RATE_ENABLE: u32 = 1 << 13;
const SLOT_TL_PARAMS_LOCKED: u32 = 1 << 14;
const SLOT_DEVICE_SCAN_TYPE: u32 = 1 << 15;
const SLOT_PACKET_SIZE: u32 = 1 << 16;
const SLOT_CHUNK_MODE: u32 = 1 << 17;
const SLOT_EXPOSURE: u32 = 1 << 18;
const SLOT_GAIN: u32 = 1 << 19;
const SLOT_REVERSE_X: u32 = 1 << 20;
const SLOT_SCAN3D_SELECTOR: u32 = 1 << 21;
const ALL_SLOTS: u32 = (1 << 22) - 1;

const WATCHED_FEATURES: &[(&str, u32)] = &[
    ("Width", SLOT_WIDTH),
    ("Height", SLOT_HEIGHT),
    ("OffsetX", SLOT_OFFSET_X),
    ("OffsetY", SLOT_OFFSET_Y),
    ("WidthMax", SLOT_WIDTH_MAX),
    ("HeightMax", SLOT_HEIGHT_MAX),
    ("SensorWidth", SLOT_SENSOR_WIDTH),
    ("SensorHeight", SLOT_SENSOR_HEIGHT),
    ("PixelFormat", SLOT_PIXEL_FORMAT),
    ("PayloadSize", SLOT_PAYLOAD_SIZE),
    ("AcquisitionStart", SLOT_ACQ_START),
    ("AcquisitionStop", SLOT_ACQ_STOP),
    ("AcquisitionFrameRate", SLOT_FRAME_RATE),
    ("AcquisitionFrameRateAbs", SLOT_FRAME_RATE),
    ("AcquisitionFrameRateEnable", SLOT_FRAME_RATE_ENABLE),
    ("TLParamsLocked", SLOT_TL_PARAMS_LOCKED),
    ("DeviceScanType", SLOT_DEVICE_SCAN_TYPE),
    ("GevSCPSPacketSize", SLOT_PACKET_SIZE),
    ("ChunkModeActive", SLOT_CHUNK_MODE),
    ("ExposureTime", SLOT_EXPOSURE),
    ("ExposureTimeAbs", SLOT_EXPOSURE),
    ("GainRaw", SLOT_GAIN),
    ("Gain", SLOT_GAIN),
    ("ReverseX", SLOT_REVERSE_X),
    ("Scan3dCoordinateSelector", SLOT_SCAN3D_SELECTOR),
];

#[derive(Debug, Clone)]
struct WatchedReg {
    node_id: Option<NodeId>,
    start: u64,
    end: u64,
    mask: u32,
    dynamic: bool,
}

/// Live GenICam evaluation model paired with a vendor [`RegisterBank`].
pub(crate) struct VendorModel {
    tree: GenApiTree,
    watched: Vec<WatchedReg>,
    has_dynamic_watched: bool,
    acq_start: (NodeId, i64),
    acq_stop: Option<(NodeId, i64)>,
    acq_start_ranges: Vec<(u64, u64)>,
    acq_stop_ranges: Vec<(u64, u64)>,
    resolved: ResolvedFeatures,
}

impl VendorModel {
    pub fn new(
        vendor: &VendorXml,
        bank: &mut RegisterBank,
        defaults: VendorDefaults,
    ) -> io::Result<Self> {
        let tree = validate_vendor_xml(vendor)?;

        // Seed register memory from XML `<Value>` defaults (and first enum entries where
        // unspecified) before resolving features.
        tree.apply_defaults(bank);

        // If `SensorWidth`/`SensorHeight`/`WidthMax`/`HeightMax`/`Width`/`Height` are still 0
        // (because the XML had no `<Value>` on their registers), seed them from `defaults`.
        for (name, fallback) in [
            ("SensorWidth", defaults.sensor_width),
            ("SensorHeight", defaults.sensor_height),
            ("WidthMax", defaults.sensor_width),
            ("HeightMax", defaults.sensor_height),
            ("Width", defaults.sensor_width.min(64)),
            ("Height", defaults.sensor_height.min(64)),
        ] {
            if let Ok(v) = tree.get_integer(bank, name) {
                if v <= 0 {
                    let _ = tree.force_set_integer(bank, name, i64::from(fallback));
                }
            }
        }
        // Seed `GevSCPSPacketSize` if present and still 0.
        if let Ok(v) = tree.get_integer(bank, "GevSCPSPacketSize") {
            if v <= 0 {
                let _ = tree.force_set_integer(bank, "GevSCPSPacketSize", 1500);
            } else {
                bank.write_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE, v as u32);
            }
        }

        let acq_start = tree
            .command_spec("AcquisitionStart")
            .expect("validated in validate_required_features");
        let acq_stop = tree.command_spec("AcquisitionStop").ok();

        let (watched, has_dynamic_watched) = Self::build_watched(&tree, bank);
        let acq_start_ranges = Self::node_ranges(&tree, bank, "AcquisitionStart");
        let acq_stop_ranges = Self::node_ranges(&tree, bank, "AcquisitionStop");

        let resolved = ResolvedFeatures {
            width: defaults.sensor_width.min(64),
            height: defaults.sensor_height.min(64),
            offset_x: 0,
            offset_y: 0,
            width_max: defaults.sensor_width,
            height_max: defaults.sensor_height,
            sensor_width: defaults.sensor_width,
            sensor_height: defaults.sensor_height,
            pixel_format: pixel_format::MONO8,
            pixel_format_name: "Mono8".to_string(),
            payload_size: 64 * 64,
            acquisition_active: false,
            frame_rate_enable: true,
            frame_rate_hz: 30.0,
            tl_params_locked: false,
            device_scan_type: scan_type::AREASCAN,
            device_scan_type_name: "Areascan".to_string(),
            packet_size: 1500,
            chunk_mode_active: false,
            exposure_time_us: 10_000,
            gain_raw: 0,
            reverse_x: false,
            scan3d_coordinate_selector: 0,
        };

        let mut model = Self {
            tree,
            watched,
            has_dynamic_watched,
            acq_start,
            acq_stop,
            acq_start_ranges,
            acq_stop_ranges,
            resolved,
        };
        model.recompute(bank, ALL_SLOTS, None);
        Ok(model)
    }

    fn build_watched(tree: &GenApiTree, bank: &mut RegisterBank) -> (Vec<WatchedReg>, bool) {
        let mut by_node: Vec<(NodeId, u32)> = Vec::new();
        for &(name, slot) in WATCHED_FEATURES {
            let Ok(regs) = tree.feature_register_nodes(name) else {
                continue;
            };
            for reg_id in regs {
                if let Some(entry) = by_node.iter_mut().find(|(id, _)| *id == reg_id) {
                    entry.1 |= slot;
                } else {
                    by_node.push((reg_id, slot));
                }
            }
        }
        let mut watched = Vec::with_capacity(by_node.len() + 1);
        let mut any_dynamic = false;
        for (node_id, mask) in by_node {
            if let Ok(Some((addr, len, dynamic))) = tree.register_range(bank, node_id) {
                any_dynamic |= dynamic;
                watched.push(WatchedReg {
                    node_id: Some(node_id),
                    start: addr,
                    end: addr.saturating_add(u64::from(len)),
                    mask,
                    dynamic,
                });
            }
        }
        // Always watch the bootstrap stream packet-size register (0x0d04).
        watched.push(WatchedReg {
            node_id: None,
            start: u64::from(offset::STREAM_CHANNEL_0_PACKET_SIZE),
            end: u64::from(offset::STREAM_CHANNEL_0_PACKET_SIZE) + 4,
            mask: SLOT_PACKET_SIZE,
            dynamic: false,
        });
        (watched, any_dynamic)
    }

    fn node_ranges(tree: &GenApiTree, bank: &mut RegisterBank, name: &str) -> Vec<(u64, u64)> {
        let Ok(regs) = tree.feature_register_nodes(name) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for id in regs {
            if let Ok(Some((addr, len, _))) = tree.register_range(bank, id) {
                out.push((addr, addr.saturating_add(u64::from(len))));
            }
        }
        out
    }

    pub fn resolved(&self) -> &ResolvedFeatures {
        &self.resolved
    }

    /// Check whether a write of `data` at `address` should be refused with
    /// `INVALID_PARAMETER` (for example, writing an unavailable `PixelFormat` entry in the
    /// current `DeviceScanType`).
    pub fn refuses_write(&mut self, bank: &mut RegisterBank, address: u32, data: &[u8]) -> bool {
        let w_start = u64::from(address);
        let w_end = w_start + data.len() as u64;
        let touches_pf = self
            .watched
            .iter()
            .any(|w| (w.mask & SLOT_PIXEL_FORMAT) != 0 && w_start < w.end && w_end > w.start);
        if !touches_pf {
            return false;
        }
        // Tentatively apply the write to check whether `PixelFormat` resolves to an available
        // enum entry; restore the previous bytes if not.
        let prev = bank.read(address, data.len());
        bank.write(address, data);
        self.tree.invalidate_cache();
        let valid = match (
            self.tree.get_enum_symbolic(bank, "PixelFormat"),
            self.tree.feature_info(bank, "PixelFormat"),
        ) {
            (Ok(sym), Ok(info)) => info
                .entries
                .as_ref()
                .and_then(|entries| entries.iter().find(|e| e.name == sym))
                .map(|e| e.available)
                .unwrap_or(true),
            (Err(_), _) => false,
            _ => true,
        };
        if !valid {
            bank.write(address, &prev);
            self.tree.invalidate_cache();
            return true;
        }
        bank.write(address, &prev);
        self.tree.invalidate_cache();
        false
    }

    /// Re-evaluate any SFNC features whose backing registers (or invalidators) overlap
    /// `[address, address + len)`. Unrelated register writes return immediately without
    /// evaluating the [`GenApiTree`].
    pub fn after_write(&mut self, bank: &mut RegisterBank, address: u32, len: usize) {
        let w_start = u64::from(address);
        let w_end = w_start + len as u64;
        let mut dirty = 0u32;
        for w in &self.watched {
            if w_start < w.end && w_end > w.start {
                dirty |= w.mask;
            }
        }
        if dirty == 0 {
            return;
        }
        self.tree.invalidate_cache();
        if self.has_dynamic_watched {
            for w in &mut self.watched {
                if w.dynamic {
                    if let Some(id) = w.node_id {
                        if let Ok(Some((addr, l, _))) = self.tree.register_range(bank, id) {
                            w.start = addr;
                            w.end = addr.saturating_add(u64::from(l));
                        }
                    }
                }
            }
        }
        self.recompute(bank, dirty, Some((w_start, w_end)));
    }

    /// Force a full re-evaluation of all tracked SFNC features.
    pub fn refresh_all(&mut self, bank: &mut RegisterBank) {
        self.tree.invalidate_cache();
        self.recompute(bank, ALL_SLOTS, None);
    }

    fn recompute(&mut self, bank: &mut RegisterBank, dirty: u32, write_range: Option<(u64, u64)>) {
        if (dirty & SLOT_SENSOR_WIDTH) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "SensorWidth") {
                if v > 0 {
                    self.resolved.sensor_width = v as u32;
                }
            }
        }
        if (dirty & SLOT_SENSOR_HEIGHT) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "SensorHeight") {
                if v > 0 {
                    self.resolved.sensor_height = v as u32;
                }
            }
        }
        if (dirty & (SLOT_WIDTH_MAX | SLOT_SENSOR_WIDTH)) != 0 {
            self.resolved.width_max = self
                .tree
                .get_integer(bank, "WidthMax")
                .ok()
                .filter(|&v| v > 0)
                .map(|v| v as u32)
                .unwrap_or(self.resolved.sensor_width);
        }
        if (dirty & (SLOT_HEIGHT_MAX | SLOT_SENSOR_HEIGHT)) != 0 {
            self.resolved.height_max = self
                .tree
                .get_integer(bank, "HeightMax")
                .ok()
                .filter(|&v| v > 0)
                .map(|v| v as u32)
                .unwrap_or(self.resolved.sensor_height);
        }
        if (dirty & SLOT_WIDTH) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "Width") {
                if v > 0 {
                    self.resolved.width = v as u32;
                }
            }
        }
        if (dirty & SLOT_HEIGHT) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "Height") {
                if v > 0 {
                    self.resolved.height = v as u32;
                }
            }
        }
        if (dirty & SLOT_OFFSET_X) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "OffsetX") {
                self.resolved.offset_x = v.max(0) as u32;
            }
        }
        if (dirty & SLOT_OFFSET_Y) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "OffsetY") {
                self.resolved.offset_y = v.max(0) as u32;
            }
        }
        if (dirty & SLOT_DEVICE_SCAN_TYPE) != 0 {
            let prev_scan = self.resolved.device_scan_type;
            if let Ok(sym) = self.tree.get_enum_symbolic(bank, "DeviceScanType") {
                self.resolved.device_scan_type = if sym.eq_ignore_ascii_case("Linescan3D") {
                    scan_type::LINESCAN3D
                } else {
                    scan_type::AREASCAN
                };
                self.resolved.device_scan_type_name = sym;
            } else if let Ok(v) = self.tree.get_integer(bank, "DeviceScanType") {
                self.resolved.device_scan_type = v as u32;
                self.resolved.device_scan_type_name =
                    if self.resolved.device_scan_type == scan_type::LINESCAN3D {
                        "Linescan3D".to_string()
                    } else {
                        "Areascan".to_string()
                    };
            }
            // If switching `DeviceScanType` made the current `PixelFormat` entry unavailable,
            // move `PixelFormat` to the first available entry in the new scan mode.
            if self.resolved.device_scan_type != prev_scan {
                if let Ok(info) = self.tree.feature_info(bank, "PixelFormat") {
                    if let Some(entries) = info.entries {
                        let current_ok = self
                            .tree
                            .get_enum_symbolic(bank, "PixelFormat")
                            .ok()
                            .and_then(|cur| entries.iter().find(|e| e.name == cur))
                            .map(|e| e.available)
                            .unwrap_or(false);
                        if !current_ok {
                            if let Some(first_avail) = entries.iter().find(|e| e.available) {
                                let _ = self.tree.force_set_enum_symbolic(
                                    bank,
                                    "PixelFormat",
                                    &first_avail.name,
                                );
                            }
                        }
                    }
                }
            }
        }
        if (dirty & (SLOT_PIXEL_FORMAT | SLOT_DEVICE_SCAN_TYPE)) != 0 {
            if let Ok(sym) = self.tree.get_enum_symbolic(bank, "PixelFormat") {
                self.resolved.pixel_format_name = sym;
            }
            if let Ok(v) = self.tree.get_integer(bank, "PixelFormat") {
                self.resolved.pixel_format = v as u32;
            }
        }
        if (dirty & SLOT_CHUNK_MODE) != 0 {
            if let Ok(b) = self.tree.get_boolean(bank, "ChunkModeActive") {
                self.resolved.chunk_mode_active = b;
            }
        }

        // Keep PayloadSize in sync whenever Width, Height, PixelFormat, ChunkModeActive, or
        // PayloadSize itself changes.
        if (dirty
            & (SLOT_WIDTH
                | SLOT_HEIGHT
                | SLOT_PIXEL_FORMAT
                | SLOT_DEVICE_SCAN_TYPE
                | SLOT_CHUNK_MODE
                | SLOT_PAYLOAD_SIZE))
            != 0
        {
            let bpp =
                bytes_per_pixel_for(&self.resolved.pixel_format_name, self.resolved.pixel_format);
            let computed = self
                .resolved
                .width
                .saturating_mul(self.resolved.height)
                .saturating_mul(bpp)
                .saturating_add(if self.resolved.chunk_mode_active {
                    CHUNK_MODE_EXTRA_BYTES
                } else {
                    0
                });
            // If PayloadSize is backed by a register, update that register so reading PayloadSize
            // through the XML matches the payload the emulator sends.
            let _ = self
                .tree
                .force_set_integer(bank, "PayloadSize", i64::from(computed));
            self.resolved.payload_size = self
                .tree
                .get_integer(bank, "PayloadSize")
                .ok()
                .filter(|&v| v > 0)
                .map(|v| v as u32)
                .unwrap_or(computed);
        }

        // Handle AcquisitionStart / AcquisitionStop command writes.
        if (dirty & (SLOT_ACQ_START | SLOT_ACQ_STOP)) != 0 {
            let touched_start = write_range
                .map(|(ws, we)| {
                    self.acq_start_ranges
                        .iter()
                        .any(|&(rs, re)| ws < re && we > rs)
                })
                .unwrap_or(true);
            let touched_stop = write_range
                .map(|(ws, we)| {
                    self.acq_stop_ranges
                        .iter()
                        .any(|&(rs, re)| ws < re && we > rs)
                })
                .unwrap_or(false);

            let (start_node, start_val) = self.acq_start;
            if let Some((stop_node, stop_val)) = self.acq_stop {
                if stop_node != start_node {
                    if touched_stop
                        && self.tree.eval_node_int(bank, stop_node).ok() == Some(stop_val)
                    {
                        self.resolved.acquisition_active = false;
                        let _ = self.tree.force_write_node_int(bank, stop_node, 0);
                        let _ = self.tree.force_write_node_int(bank, start_node, 0);
                    } else if touched_start
                        && self.tree.eval_node_int(bank, start_node).ok() == Some(start_val)
                    {
                        self.resolved.acquisition_active = true;
                    }
                } else if let Ok(v) = self.tree.eval_node_int(bank, start_node) {
                    if v == start_val
                        && (start_val != stop_val || !self.resolved.acquisition_active)
                    {
                        self.resolved.acquisition_active = true;
                    } else if v == stop_val {
                        self.resolved.acquisition_active = false;
                    }
                }
            } else if let Ok(v) = self.tree.eval_node_int(bank, start_node) {
                self.resolved.acquisition_active = v == start_val;
            }

            let _ = self.tree.force_set_integer(
                bank,
                "TLParamsLocked",
                i64::from(self.resolved.acquisition_active),
            );
        }

        if (dirty & (SLOT_TL_PARAMS_LOCKED | SLOT_ACQ_START | SLOT_ACQ_STOP)) != 0 {
            self.resolved.tl_params_locked = self
                .tree
                .get_integer(bank, "TLParamsLocked")
                .map(|v| v != 0)
                .unwrap_or(self.resolved.acquisition_active);
        }

        if (dirty & SLOT_FRAME_RATE_ENABLE) != 0 {
            if let Ok(b) = self.tree.get_boolean(bank, "AcquisitionFrameRateEnable") {
                self.resolved.frame_rate_enable = b;
            }
        }
        if (dirty & SLOT_FRAME_RATE) != 0 {
            let fps = self
                .tree
                .get_float(bank, "AcquisitionFrameRate")
                .or_else(|_| self.tree.get_float(bank, "AcquisitionFrameRateAbs"))
                .or_else(|_| {
                    self.tree
                        .get_integer(bank, "AcquisitionFrameRate")
                        .map(|v| v as f64)
                });
            if let Ok(f) = fps {
                if f.is_finite() && f > 0.0 {
                    self.resolved.frame_rate_hz = f as f32;
                }
            }
        }

        if (dirty & SLOT_PACKET_SIZE) != 0 {
            let boot_addr = u64::from(offset::STREAM_CHANNEL_0_PACKET_SIZE);
            let wrote_boot = write_range
                .map(|(ws, we)| ws < boot_addr + 4 && we > boot_addr)
                .unwrap_or(false);
            if wrote_boot {
                let pkt = bank.read_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE) & 0xffff;
                if pkt > 0 {
                    self.resolved.packet_size = pkt;
                    let _ = self
                        .tree
                        .force_set_integer(bank, "GevSCPSPacketSize", i64::from(pkt));
                }
            } else if let Ok(v) = self.tree.get_integer(bank, "GevSCPSPacketSize") {
                let pkt = (v as u32) & 0xffff;
                if pkt > 0 {
                    self.resolved.packet_size = pkt;
                    bank.write_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE, pkt);
                }
            } else {
                let pkt = bank.read_u32(offset::STREAM_CHANNEL_0_PACKET_SIZE) & 0xffff;
                if pkt > 0 {
                    self.resolved.packet_size = pkt;
                }
            }
        }

        if (dirty & SLOT_EXPOSURE) != 0 {
            let exp = self
                .tree
                .get_float(bank, "ExposureTime")
                .or_else(|_| self.tree.get_float(bank, "ExposureTimeAbs"))
                .or_else(|_| {
                    self.tree
                        .get_integer(bank, "ExposureTime")
                        .map(|v| v as f64)
                });
            if let Ok(e) = exp {
                if e.is_finite() && e >= 0.0 {
                    self.resolved.exposure_time_us = e as u32;
                }
            }
        }
        if (dirty & SLOT_GAIN) != 0 {
            let gain = self
                .tree
                .get_integer(bank, "GainRaw")
                .or_else(|_| self.tree.get_float(bank, "Gain").map(|v| v as i64));
            if let Ok(g) = gain {
                self.resolved.gain_raw = g.max(0) as u32;
            }
        }
        if (dirty & SLOT_REVERSE_X) != 0 {
            if let Ok(b) = self.tree.get_boolean(bank, "ReverseX") {
                self.resolved.reverse_x = b;
            }
        }
        if (dirty & SLOT_SCAN3D_SELECTOR) != 0 {
            if let Ok(v) = self.tree.get_integer(bank, "Scan3dCoordinateSelector") {
                self.resolved.scan3d_coordinate_selector = v.max(0) as u32;
            }
        }
    }

    pub fn set_region(
        &mut self,
        bank: &mut RegisterBank,
        offset_x: u32,
        offset_y: u32,
        width: u32,
        height: u32,
    ) {
        let _ = self
            .tree
            .force_set_integer(bank, "OffsetX", i64::from(offset_x));
        let _ = self
            .tree
            .force_set_integer(bank, "OffsetY", i64::from(offset_y));
        let _ = self.tree.force_set_integer(bank, "Width", i64::from(width));
        let _ = self
            .tree
            .force_set_integer(bank, "Height", i64::from(height));
        self.refresh_all(bank);
    }

    pub fn set_pixel_format_symbolic(
        &mut self,
        bank: &mut RegisterBank,
        symbolic: &str,
    ) -> Result<(), String> {
        self.tree
            .force_set_enum_symbolic(bank, "PixelFormat", symbolic)
            .map_err(|e| e.to_string())?;
        self.refresh_all(bank);
        Ok(())
    }

    pub fn set_device_scan_type_symbolic(
        &mut self,
        bank: &mut RegisterBank,
        symbolic: &str,
    ) -> Result<(), String> {
        self.tree
            .force_set_enum_symbolic(bank, "DeviceScanType", symbolic)
            .map_err(|e| e.to_string())?;
        self.recompute(
            bank,
            SLOT_DEVICE_SCAN_TYPE | SLOT_PIXEL_FORMAT | SLOT_PAYLOAD_SIZE,
            None,
        );
        Ok(())
    }

    pub fn set_frame_rate(&mut self, bank: &mut RegisterBank, enabled: bool, hz: f32) {
        let _ = self
            .tree
            .force_set_integer(bank, "AcquisitionFrameRateEnable", i64::from(enabled));
        if self
            .tree
            .force_set_float(bank, "AcquisitionFrameRate", f64::from(hz))
            .is_err()
        {
            let _ = self
                .tree
                .force_set_float(bank, "AcquisitionFrameRateAbs", f64::from(hz));
        }
        self.resolved.frame_rate_enable = enabled;
        self.resolved.frame_rate_hz = hz;
        self.refresh_all(bank);
    }

    pub fn stop_acquisition(&mut self, bank: &mut RegisterBank) {
        let (start_node, _) = self.acq_start;
        let _ = self.tree.force_write_node_int(bank, start_node, 0);
        if let Some((stop_node, stop_val)) = self.acq_stop {
            if stop_node == start_node {
                let _ = self.tree.force_write_node_int(bank, stop_node, stop_val);
            } else {
                let _ = self.tree.force_write_node_int(bank, stop_node, 0);
            }
        }
        self.resolved.acquisition_active = false;
        let _ = self.tree.force_set_integer(bank, "TLParamsLocked", 0);
        self.resolved.tl_params_locked = false;
    }
}

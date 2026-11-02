# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/) (before 1.0, a breaking change bumps the minor version).
All crates in the workspace share one version.

## [Unreleased]

### Added

- Fake camera as a networked device: `FakeCameraConfig::gvcp_bind` (default `127.0.0.1:0`) or
  `FakeCamera::start_on` with an already bound socket, and `gvsp_source_ip` (default: the GVCP
  IP) for stream, resend and test packets. `CurrentIPAddress` and the discovery ack report the
  bound IP.
- Fake camera: `discovery_netmask` answers discovery only from requesters in the camera's
  subnet (default: everyone).
- Fake camera: a `FrameSource` trait (also implemented for closures) is asked for each frame's
  payload with the region, pixel format (value and name), frame id and timestamp.
  `PatternSource`, the built-in patterns, is the default, and `FakeCamera::set_frame_source`
  swaps the source while streaming.
- Fake camera: with `pace_from_frame_rate`, frames are paced from `AcquisitionFrameRate` while
  `AcquisitionFrameRateEnable` is set. `frame_period` stays the default.
- Fake camera: `sensor_width`/`sensor_height` (default 4096 × 4096) set what `WidthMax` and
  `HeightMax` report.
- Fake camera: vendor GenICam XML mode (`FakeCameraConfig::genicam_xml: Option<VendorXml>`,
  `validate_vendor_xml`, `FakeCamera::resolved_features`). Serves raw `.xml` or `.zip` blobs at a
  configurable 32-bit address (`DEFAULT_VENDOR_XML_ADDRESS = 0x0100_0000`), backs the 32-bit
  register space with sparse memory, seeds `<Value>` defaults (`GenApiTree::apply_defaults`), and
  resolves SFNC features (`Width`, `Height`, `OffsetX`, `OffsetY`, `WidthMax`, `HeightMax`,
  `SensorWidth`, `SensorHeight`, `PixelFormat`, `PayloadSize`, `AcquisitionStart`/
  `AcquisitionStop`, `AcquisitionFrameRate(Enable)`, `TLParamsLocked`, `DeviceScanType`,
  `GevSCPSPacketSize`) through `aravis-port-genicam` on writes touching their backing registers.
- Fake camera: `Scan3dCoordinateScale`/`Scan3dCoordinateOffset` of CoordinateA/B/C are backed
  by registers (`feature::SCAN3D_COORDINATE_SCALE`/`_OFFSET`), and
  `FakeCamera::set_scan3d_calibration` sets them. The power-on values are the former fixed ones
  (`SCAN3D_DEFAULT_SCALES`: A 0.05, B 0.1, C 0.001 mm, offsets 0), now as `f32`.
- `aravis-port-core::zip`: single-file `.zip`/DEFLATE helpers (`extract_first_file`, `is_zip`,
  `store_single_file`, `deflate_single_file`), shared by `aravis-port-device` and
  `aravis-port-fakecamera`.
- `GenApiTree::parse_located` returns a `LocatedError` with the byte offset of an XML syntax
  error or of the node that could not be built, and `LocatedError::line_column`. The DOM records
  each element's offset (`XmlElement::offset`).
- Fake camera: `required_feature_problems` lists every required SFNC feature a vendor XML lacks
  or mistypes (`validate_vendor_xml` still fails on the first), and `followed_optional_features`
  the optional ones the fake camera follows. `minimal_genicam_xml` (the built-in XML) is public.

### Changed

- GenICam parsing rejects a document with an unclosed element instead of accepting it.

- Fake camera: resends replay the last `resend_history` (default 16) frames as sent, instead of
  regenerating the pattern from the current registers. Older frames are answered with "packet
  unavailable". Frames are due one period after the previous one rather than one period after
  the previous send finished.

## [0.2.0] - 2026-10-08

First release of the umbrella crate `aravis-port` and of `aravis-port-device` and
`aravis-port-fakecamera`.

### Changed

- **Breaking:** `aravis-port-memory` is folded into `aravis-port-core` (as `core::memory`), and
  `aravis-port-net` into `aravis-port-device` (as `device::net`). Both old crates are retired. The
  umbrella crate still exposes `aravis_port::memory` and `aravis_port::net`.
- `discover` applies its timeout per bind address, so a multi-NIC discovery round no longer
  returns only the first interface's cameras. A round now takes up to `n * timeout`.
- `Camera::open_stream` programs `GevSCPSPacketSize` before reading `PayloadSize` (the AT C6's
  payload size depends on it).
- Minimum supported Rust version is 1.82.

### Added

- Umbrella API: `Camera::device()`/`device_mut()`, the `Device`/`DeviceConfig`/`FeatureValue` and
  buffer-pool re-exports, `StreamConfig`, and `start_stream_with_config` /
  `start_stream_channel_with_config`.
- `Device::genicam_xml()` and `feature_kind()` (also on `Camera`).
- GenICam: `String` and `StructReg`/`StructEntry` nodes, `pIsAvailable` (`is_available`),
  `pIsLocked` (`is_locked`, `lock_depends_on`), `feature_info` with enumeration entry states, and
  `invalidate_cache`.
- Chunk data: `ChunkDataAccess` in genicam and `Camera::read_chunk`.
- GVSP multi-part payloads (content type 7), as streamed by the AT C6.
- Packet size: `Camera::stream_packet_size`, `test_packet_size` and `auto_packet_size`
  (`PacketSizeSearch`/`PacketSizeOutcome`), and on `Device` the don't-fragment flag and test-packet
  firing (`bootstrap::stream_packet_size` bits).
- GenICam 1.0 legacy register access (READREG/WRITEREG for 4-byte features) for schema < 1.1.0 and
  quirk-listed devices such as the C6.
- `LiveCamIntegrationTests` (`aravis-port/tests/live_cam_integration_tests.rs`), run against a real
  camera with `--serial <SN>`.
- Fake camera: Mono10/Mono16 patterns, `OffsetX`/`OffsetY`/`WidthMax`/`HeightMax`, frame rate,
  software trigger, `ReverseX`, `GevSCPSPacketSize` with a path MTU and test packets,
  `resend_unavailable`, and a C6-like `Linescan3D` mode streaming `Coord3D_C16` profiles.

### Fixed

- The GVSP payload stride now accounts for the IP and UDP headers inside `GevSCPSPacketSize`.
  Before, frames carried 28-byte zero gaps per packet while still closing as `Success`.
- A resend answered with a data-less "packet unavailable" error packet is no longer counted as
  received, so such a frame closes as `MissingPackets` instead of `Success` with a short tail.
- Several GenICam node evaluation fixes.

## [0.1.0] - 2026-07-23

Initial release of `aravis-port-core`, `aravis-port-genicam`, `aravis-port-stream`,
`aravis-port-memory` and `aravis-port-net`.

[Unreleased]: https://github.com/javerik/aravis-port/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/javerik/aravis-port/compare/7293f66...v0.2.0
[0.1.0]: https://github.com/javerik/aravis-port/commit/7293f66

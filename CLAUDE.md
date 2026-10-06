# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A pure-Rust, zero-`unsafe` port of the GigE Vision (GEV) subset of Aravis 0.9.2: discovery, GVCP control, GenICam XML/node-tree evaluation, and GVSP streaming. The original spec is referenced as `ai/rust-port.md` in the README and code comments, but `ai/` is gitignored and may not exist locally.

## Commands

```sh
cargo build --workspace
cargo test --workspace --all-features                  # full suite, runs against the in-process fake camera; no hardware needed
cargo clippy --workspace --all-targets -- -D warnings  # must be clean
cargo test -p aravis-port-genicam                      # one crate
cargo test -p aravis-port --test integration           # one integration-test file
cargo test -p aravis-port-stream --test resend_under_loss <test_name>   # a single test
cargo test -p aravis-port --test live_cam_integration_tests -- --serial <SN>  # LiveCamIntegrationTests against a real camera; skipped without --serial
```

The live-hardware examples (`live_check*`, `discover`) under `aravis-port-device/examples/` and `aravis-port/examples/` need a real camera on the network and are not part of `cargo test`. Run one with `cargo run -p <crate> --example <name> -- <camera-ip>`. The header comment of each file lists its arguments.

`aravis-port/tests/live_cam_integration_tests.rs` (`LiveCamIntegrationTests`) is the asserting counterpart. It uses its own std-only runner (`harness = false`) so it can accept `--serial <SN>`. Tests run sequentially, each on a freshly connected `Camera`, and each must restore any feature it writes before returning its result.

## Hard constraints

- **`#![forbid(unsafe_code)]` in every crate.** Don't add `unsafe`, even behind a feature flag.
- **The dependency list is closed.** The only external crates are the ones in `[workspace.dependencies]` (`quick-xml`, `thiserror`, `log`, `bitflags`, `rand`, `crc32fast`, and `env_logger` for dev). This is why the crate has its own DEFLATE/zip decoder (`aravis-port-device/src/zip/`), why discovery needs explicit bind addresses (std can't enumerate NICs), and why `SO_RCVBUF` isn't set (that would need `socket2`). Implement features with `std` rather than adding a crate.
- **Parsers must return errors on malformed input, not panic.** Every wire/XML/formula/deflate parser has `*_is_an_error_not_a_panic`/`truncated_*` tests. Keep that pattern when adding decoders.
- **Publishing:** internal *dev*-dependencies between crates stay path-only with no `version` (device and stream use each other as dev-deps, and a version would create a publish cycle). Real dependencies go through `[workspace.dependencies]` with both `path` and `version`. The README gives the required `cargo publish` order.

## Architecture

Six crates, layered bottom-up:

- **`aravis-port-core`**: wire formats only, with no I/O. GVCP header/messages, GVSP header/leader/trailer, GVBS bootstrap register offsets (`bootstrap::offset`), `MacAddress`, and the shared `Error`/`Result`. Also has `core::memory`: `Buffer`, the buffer pool (`new_buffer_pool` returns a user-side `BufferPoolHandle` and a `BufferPoolStreamSide`), and the chunk-data TLV index.
- **`aravis-port-genicam`**: the GenICam engine, which is transport-agnostic. `GenApiTree::parse(xml)` builds an arena of `Node`s addressed by `NodeId`, and handles the formula lexer/parser/evaluator (for SwissKnife/Converter) and register caching and invalidation. Every method that touches registers takes `&mut impl RegisterAccess`, a two-method read/write-memory trait. Tests use `MemoryRegisterAccess` (a `Vec<u8>`) instead of sockets. The node kinds this crate can build are listed in `CONSTRUCTIBLE_TAGS` in `tree.rs`. Unmodeled kinds resolve as inert placeholders so documents that use them still parse.
- **`aravis-port-device`**: the GVCP client. `net::GvcpTransaction` is the request/ack/retry state machine, and `net::discover` handles broadcast discovery. `Device::connect` takes exclusive control-channel privilege, fetches the GenICam XML through the bootstrap URL registers (unzipping it if needed), parses it, and spawns a heartbeat thread. The transaction sits in an `Arc<Mutex<_>>` shared by foreground calls and the heartbeat. `io::GvcpTransactionIo` adapts it to `RegisterAccess`. `FeatureValue` (implemented for `i64`/`f64`/`bool`/`String`) maps `device.read::<T>(name)` to the right typed tree call. `String` dispatches on node kind (Enumeration symbolic vs StringReg).
- **`aravis-port-stream`**: the GVSP receiver thread and `Reassembler`. It deliberately does **not** depend on `aravis-port-device`: it only needs a raw `UdpSocket`, and its one outbound need (packet resend) goes through the `ResendRequester` trait.
- **`aravis-port`**: the umbrella/public API. It re-exports the sub-crates (`aravis_port::core`, `genicam`, `device`, `stream`, plus `memory` from core and `net` from device) and provides `discover()`, `Camera`, and `prelude`. `Camera::open_stream` wires everything together: it resolves the local route to the camera, binds the GVSP socket, programs the stream channel and `GevSCPSPacketSize` (this overwrites any earlier packet-size write), creates the pool, connects `ResendRequester` to the device's `PacketResendSender`, and runs `AcquisitionStart` as a best-effort step.
- **`aravis-port-fakecamera`**: an in-process GVCP+GVSP simulator on loopback ephemeral ports (not 3956). It serves a hand-authored GenICam XML (`src/xml.rs`) backed by a flat register bank, with custom feature registers at `registers::feature::*` and the XML mapped at addresses `>= REGISTER_SPACE_SIZE`. It can drop GVSP packets (`gvsp_loss_probability`) to test resends. `DeviceScanType = Linescan3D` models a C6-like 3D profiler: the only `PixelFormat` is then `Coord3D_C16` (a profile per row, height per value, continuing across frames), the GVCP server refuses a `PixelFormat` write the current scan type doesn't offer (the GenICam client writes unavailable entries anyway), and a scan-type switch moves `PixelFormat` onto one that it does. Integration tests in `device`, `stream` and `aravis-port` run against it, so `FakeCamera::start(cfg)` then `Camera::connect_addr(camera.local_addr())` (or `Device::connect`) is the standard test setup. Features that need new camera behavior usually need matching changes to the fake camera's XML and registers.

### Subtleties learned from real hardware (don't "simplify" these away)

These were validated against AT-Automation Technology C5-2040-GigE and C6-2040-GigE cameras, and the code comments explain each one:

- `GevSCPSPacketSize` counts IP and UDP headers too, so the per-packet payload is `packet_size - 20 - 8 - 8(status+std header)`. See `per_packet_capacity` in `stream/src/reassembly.rs`.
- The default stream packet size is 1400 because some devices' power-on default is above the 1500-byte MTU once headers are added. To stream with the device's own value instead, pass `Camera::stream_packet_size()` to `start_stream*_with_config`.
- `Camera::auto_packet_size`/`test_packet_size` port Aravis's packet-size check: write size + don't-fragment + fire-test-packet to `GevSCPSPacketSize` (bits in `bootstrap::stream_packet_size`) and wait for a UDP payload of exactly `size - 28` at a probe socket the stream channel points to. They must not run while acquiring, because stream packets of the size under test look the same. The fake camera only delivers stream and test packets up to `FakeCameraConfig::path_mtu`, and can be configured to send no test packets (`test_packets: false`).
- A resend the device can no longer serve is answered with a data-less error packet (status `0x800c`, packet unavailable). The C6 apparently types it as a plain payload packet even in a multi-part stream (inferred from a short frame of exactly 396 × the generic 7912-byte stride at packet size 7960). The reassembler skips error packets entirely, as Aravis does, so the frame closes as `MissingPackets`. Counting one as received once closed a frame as `Success` with its tail missing. The fake camera models this with `FakeCameraConfig::resend_unavailable`.
- The discovery timeout applies per bind address.
- Frame-id late-frame detection is a simple distance check and doesn't handle 16-bit wraparound. This is a documented limitation.
- Some devices need GenICam 1.0 "legacy" register access: 4-byte feature accesses go through READREG/WRITEREG instead of READMEM/WRITEMEM. This applies when the XML schema is < 1.1.0, and to devices on Aravis's quirk list even when they declare a newer schema (the C6 does). See `uses_legacy_register_access` in `device/src/io.rs`.
- The C6 streams GVSP multi-part payloads (content type 7). Each data block carries an explicit byte offset, and the final block can be padded past `PayloadSize`, so the reassembler clips it rather than rejecting it. See `ContentType::Multipart` in `stream/src/reassembly.rs`.
- The C6's `PayloadSize` depends on `GevSCPSPacketSize`, and `PayloadSizeReg` is cached without that register as an invalidator. So `Camera::open_stream` programs the packet size *before* reading `PayloadSize`, and raw `Device::write_register`/`write_memory` drop the GenICam cache.
- Chunk data: in chunk mode the device wraps the image itself as the first chunk, so the whole payload is a `[data][id][size]` chain that `ChunkTlvIndex` walks from the end. Chunk features are registers behind a `<Port>` with a `<ChunkID>`, usually indexed by a selector through `pIndex`. Read them with `Device::read_chunk`/`Camera::read_chunk` (genicam `ChunkDataAccess`).
- `Device::connect` must release control-channel privilege when it fails after acquiring it (e.g. a GenICam parse error). Otherwise the camera refuses every client until its heartbeat timeout expires.

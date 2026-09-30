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
```

The live-hardware examples (`live_check*`, `discover`) under `aravis-port-device/examples/` and `aravis-port/examples/` need a real camera on the network and are not part of `cargo test`. Run one with `cargo run -p <crate> --example <name> -- <camera-ip>`. The header comment of each file lists its arguments.

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
- **`aravis-port-fakecamera`**: an in-process GVCP+GVSP simulator on loopback ephemeral ports (not 3956). It serves a hand-authored GenICam XML (`src/xml.rs`) backed by a flat register bank, with custom feature registers at `registers::feature::*` and the XML mapped at addresses `>= REGISTER_SPACE_SIZE`. It can drop GVSP packets (`gvsp_loss_probability`) to test resends. Integration tests in `device`, `stream` and `aravis-port` run against it, so `FakeCamera::start(cfg)` then `Camera::connect_addr(camera.local_addr())` (or `Device::connect`) is the standard test setup. Features that need new camera behavior usually need matching changes to the fake camera's XML and registers.

### Subtleties learned from real hardware (don't "simplify" these away)

These were validated against an AT-Automation Technology C5-2040-GigE, and the code comments explain each one:

- `GevSCPSPacketSize` counts IP and UDP headers too, so the per-packet payload is `packet_size - 20 - 8 - 8(status+std header)`. See `per_packet_capacity` in `stream/src/reassembly.rs`.
- The default stream packet size is 1400 because some devices' power-on default is above the 1500-byte MTU once headers are added.
- The discovery timeout applies per bind address.
- Frame-id late-frame detection is a simple distance check and doesn't handle 16-bit wraparound. This is a documented limitation.

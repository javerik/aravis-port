# aravis-port

[![CI](https://github.com/javerik/aravis-port/actions/workflows/ci.yml/badge.svg)](https://github.com/javerik/aravis-port/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/aravis-port.svg)](https://crates.io/crates/aravis-port)
[![docs.rs](https://img.shields.io/docsrs/aravis-port)](https://docs.rs/aravis-port)

A pure-Rust, zero-`unsafe` port of the GigE Vision (GEV) subset of [Aravis](https://github.com/aravis-project/aravis) 0.9.2.

```toml
[dependencies]
aravis-port = "0.2"
```

The minimum supported Rust version is 1.82. Changes are listed in [`CHANGELOG.md`](CHANGELOG.md).

```rust
use aravis_port::prelude::*;
use std::time::Duration;

let cameras = aravis_port::discover(Duration::from_secs(2))?;
let camera = Camera::new(&cameras[0])?;

let exposure = camera.read::<f64>("ExposureTime")?;

let stream = camera.start_stream(|buffer| {
    println!("Received frame id {} size {}", buffer.frame_id, buffer.data().len());
})?;

std::thread::sleep(Duration::from_secs(5));
camera.stop_stream(stream)?;
```

## Status

All six phases of the spec are implemented and validated both against an in-process fake camera and real AT-Automation Technology C5-2040-GigE and C6-2040-GigE cameras on the network: discovery, GVCP register/memory read-write, GenICam XML fetch (including zip-compressed XML, via a from-scratch pure-Rust DEFLATE decoder) and node-tree evaluation (including the formula evaluator and masked-register bit layout), GVSP streaming with packet reassembly/resend, and the umbrella `Camera` API.

```
cargo test --workspace --all-features   # unit + fakecamera integration tests
cargo clippy --workspace --all-targets -- -D warnings
```

No `unsafe` appears anywhere in the workspace (`#![forbid(unsafe_code)]` in every crate).

## Workspace layout

| Crate | Responsibility |
|---|---|
| `aravis-port-core` | GVCP/GVSP packet structs, GVBS bootstrap register offsets, `MacAddress`, shared `Error`; `core::memory`: `Buffer`, buffer pool, chunk-data TLV index |
| `aravis-port-genicam` | XML → node-tree engine, formula evaluator, register access trait |
| `aravis-port-device` | GVCP device client, GenICam feature bridge, heartbeat, XML fetch (incl. unzip); `device::net`: UDP discovery, GVCP request/response transaction state machine |
| `aravis-port-stream` | GVSP receiver thread, packet reassembly/resend |
| `aravis-port` | Umbrella crate — `discover()`, `Camera`, the public API |
| `aravis-port-fakecamera` | In-process GVCP/GVSP simulator used by the test suite |

`aravis-port-memory` and `aravis-port-net` were folded into `core`/`device` respectively — each existed only because of the original 8-crate spec, not for any load-bearing architectural reason. The umbrella crate still exposes `aravis_port::memory::*` and `aravis_port::net::*` unchanged.

## Releasing

All crates share the version in `[workspace.package]` and are released together:

1. Bump `version` in `[workspace.package]` and the internal entries in `[workspace.dependencies]`.
2. Move the `Unreleased` entries in `CHANGELOG.md` under a `## [X.Y.Z] - date` heading.
3. Commit, push, wait for CI, then tag: `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`.

The tag runs `.github/workflows/release.yml`. It reruns CI, checks the tag against the workspace version, publishes the crates to crates.io in dependency order through `scripts/publish-crates.sh` (it skips versions already on crates.io, so a failed release can be rerun), and creates a GitHub Release with the changelog section and the packaged `.crate` files. It needs the repository secret `CARGO_REGISTRY_TOKEN`.

To check packaging locally, run `cargo publish --workspace --dry-run` (Cargo 1.90 or newer resolves the unpublished internal dependencies through a temporary registry) or `DRY_RUN=1 scripts/publish-crates.sh`.

Workspace-internal *dev*-dependencies (e.g. `aravis-port-device` and `aravis-port-stream` use each other as test fixtures) are deliberately path-only with no `version`, so Cargo drops them from the published manifest instead of creating a publish cycle.

## Live-hardware checks

Each of `aravis-port-device` and `aravis-port` has a `live_check_*` example under its `examples/` directory, used during development to validate against a real camera on the network (not part of `cargo test`, since CI has no camera attached). Run with `cargo run -p <crate> --example <name> -- <args>`; see each file for its expected arguments.

For asserting end-to-end coverage against a real camera, `aravis-port` also has the `LiveCamIntegrationTests` suite (`aravis-port/tests/live_cam_integration_tests.rs`). It runs only when given the serial number of a camera on the network, and otherwise prints a "skipped" line and passes:

```sh
cargo test -p aravis-port --test live_cam_integration_tests -- --serial <SN> [name-filter] [--bind <local-ip> --broadcast <directed-broadcast-ip>]
```

Pass `--serial` only together with `--test live_cam_integration_tests`: the other test binaries use the standard libtest harness and reject unknown flags.

## Known limitations

- **No local network interface enumeration.** `std` has no safe API for this, so [`aravis_port::net::discover`] requires an explicit bind address/directed-broadcast pair rather than scanning every NIC automatically.
- **No `SO_RCVBUF` control.** See the note in `aravis-port-device::net`'s module docs — an OS-level `sysctl` tuning concern at very high frame rates, not something this crate can address without an additional dependency.
- **GenICam node coverage** targets the practical subset real GEV cameras use (validated against the two devices above): `Category`, `Integer`, `Float`, `Boolean`, `Enumeration`, `Command`, `StringReg`, `IntReg`/`MaskedIntReg`, `StructReg`/`StructEntry`, `String`, `FloatReg`, `Converter`, `SwissKnife`, and chunk-data ports (`Camera::read_chunk`), with `pIndex` addressing and `pIsImplemented`/`pIsAvailable` (`Camera::is_available`). Less common node kinds (`IndexNode`, `Selector` as a first-class interface) are not modeled; `Port`/generic `Register` elements resolve as inert placeholders so a document referencing them still parses.

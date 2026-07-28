# aravis-port

A pure-Rust, zero-`unsafe` port of the GigE Vision (GEV) subset of [Aravis](https://github.com/aravis-project/aravis) 0.9.2. See [`ai/rust-port.md`](ai/rust-port.md) for the original specification.

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

All six phases of the spec are implemented and validated both against an in-process fake camera and a real AT-Automation Technology C5-2040-GigE camera on the network: discovery, GVCP register/memory read-write, GenICam XML fetch (including zip-compressed XML, via a from-scratch pure-Rust DEFLATE decoder) and node-tree evaluation (including the formula evaluator and masked-register bit layout), GVSP streaming with packet reassembly/resend, and the umbrella `Camera` API.

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

## Publishing

Every crate has `description`, `license`, `repository`, `readme`, `keywords`, and `categories` set (inherited from `[workspace.package]`), and every internal path dependency in `[workspace.dependencies]` carries a `version` alongside its `path` — required for `cargo publish` to resolve it once the dependency is live on crates.io. Workspace-internal *dev*-dependencies (e.g. `aravis-port-device` and `aravis-port-stream` depend on each other only as test fixtures) are deliberately left path-only with no version, so Cargo drops them from the published manifest instead of deadlocking a would-be publish cycle.

Because of the real (non-dev) dependency graph, crates must be published to crates.io in this order:

1. `aravis-port-core`
2. `aravis-port-genicam` (depends only on `core`)
3. `aravis-port-stream` (depends on `core`)
4. `aravis-port-fakecamera` (depends on `core`, `genicam`)
5. `aravis-port-device` (depends on `core`, `genicam`)
6. `aravis-port` (depends on all of the above)

For each, after the previous step's crate is confirmed live on crates.io: `cargo publish -p <crate>` (add `--dry-run` to check packaging without uploading — note a dry-run of any crate with an unpublished internal dependency will fail at the packaging step with "no matching package found," since it resolves the full graph against the live registry; this isn't a configuration problem, just the ordering constraint above).

## Live-hardware checks

Each of `aravis-port-device` and `aravis-port` has a `live_check_*` example under its `examples/` directory, used during development to validate against a real camera on the network (not part of `cargo test`, since CI has no camera attached). Run with `cargo run -p <crate> --example <name> -- <args>`; see each file for its expected arguments.

## Known limitations

- **No local network interface enumeration.** `std` has no safe API for this, so [`aravis_port::net::discover`] requires an explicit bind address/directed-broadcast pair rather than scanning every NIC automatically.
- **No `SO_RCVBUF` control.** See the note in `aravis-port-device::net`'s module docs — an OS-level `sysctl` tuning concern at very high frame rates, not something this crate can address without an additional dependency.
- **GenICam node coverage** targets the practical subset real GEV cameras use (validated against the two devices above): `Category`, `Integer`, `Float`, `Boolean`, `Enumeration`, `Command`, `StringReg`, `IntReg`/`MaskedIntReg`, `FloatReg`, `Converter`, `SwissKnife`. Less common node kinds (`StructEntry`, `IndexNode`, `Selector` as a first-class interface) are not modeled; `Port`/generic `Register` elements resolve as inert placeholders so a document referencing them still parses.

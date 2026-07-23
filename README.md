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
| `aravis-port-core` | GVCP/GVSP packet structs, GVBS bootstrap register offsets, `MacAddress`, shared `Error` |
| `aravis-port-net` | UDP discovery, GVCP request/response transaction state machine |
| `aravis-port-genicam` | XML → node-tree engine, formula evaluator, register access trait |
| `aravis-port-memory` | `Buffer`, buffer pool, chunk-data TLV index |
| `aravis-port-device` | GVCP device client, GenICam feature bridge, heartbeat, XML fetch (incl. unzip) |
| `aravis-port-stream` | GVSP receiver thread, packet reassembly/resend |
| `aravis-port` | Umbrella crate — `discover()`, `Camera`, the public API |
| `aravis-port-fakecamera` | In-process GVCP/GVSP simulator used by the test suite |

## Live-hardware checks

Each of `aravis-port-net`, `aravis-port-device`, and `aravis-port` has a `live_check_*` example under its `examples/` directory, used during development to validate against a real camera on the network (not part of `cargo test`, since CI has no camera attached). Run with `cargo run -p <crate> --example <name> -- <args>`; see each file for its expected arguments.

## Known limitations

- **No local network interface enumeration.** `std` has no safe API for this, so [`aravis_port::net::discover`] requires an explicit bind address/directed-broadcast pair rather than scanning every NIC automatically.
- **No `SO_RCVBUF` control.** See the note in `aravis-port-net`'s crate docs — an OS-level `sysctl` tuning concern at very high frame rates, not something this crate can address without an additional dependency.
- **GenICam node coverage** targets the practical subset real GEV cameras use (validated against the two devices above): `Category`, `Integer`, `Float`, `Boolean`, `Enumeration`, `Command`, `StringReg`, `IntReg`/`MaskedIntReg`, `FloatReg`, `Converter`, `SwissKnife`. Less common node kinds (`StructEntry`, `IndexNode`, `Selector` as a first-class interface) are not modeled; `Port`/generic `Register` elements resolve as inert placeholders so a document referencing them still parses.

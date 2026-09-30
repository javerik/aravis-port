//! `LiveCamIntegrationTests`: asserting end-to-end tests against a real GigE Vision camera.
//!
//! These only run when given the serial number of a camera reachable on the network; without
//! one they print a "skipped" line and exit successfully, so `cargo test --workspace` stays green
//! on machines with no camera attached. libtest rejects unknown flags, so this target uses its own
//! small runner (`harness = false` in `Cargo.toml`) that parses its own arguments:
//!
//! ```text
//! cargo test -p aravis-port --test live_cam_integration_tests -- --serial <SN> [filter]
//!     [--bind <local-ip>] [--broadcast <directed-broadcast-ip>] [--list]
//! ```
//!
//! Tests run strictly sequentially (the camera grants exclusive control to one client at a time),
//! each on its own freshly connected `Camera`. Every feature a test writes is restored before the
//! test's result is reported.

use std::net::Ipv4Addr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use aravis_port::core::bootstrap::offset;
use aravis_port::memory::PayloadType;
use aravis_port::net::{discover, DiscoveredDevice, DiscoveryOptions};
use aravis_port::prelude::*;

const GROUP: &str = "LiveCamIntegrationTests";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// What every test gets: the discovery record of the camera selected by `--serial`.
struct LiveCam {
    info: DiscoveredDevice,
}

enum Outcome {
    Passed,
    /// Passed, with values worth showing next to the result (e.g. to compare with another tool).
    PassedWith(String),
    Skipped(String),
}

type TestResult = std::result::Result<Outcome, Box<dyn std::error::Error>>;
type TestFn = fn(&LiveCam) -> TestResult;

/// Like `assert!`, but returns an error instead of panicking, so a test can still restore what it
/// wrote before reporting the failure.
macro_rules! ensure {
    ($cond:expr, $($fmt:tt)+) => {
        let holds: bool = $cond;
        if !holds {
            return Err(format!($($fmt)+).into());
        }
    };
}

const TESTS: &[(&str, TestFn)] = &[
    ("discovery_finds_camera_by_serial", discovery_finds_camera_by_serial),
    ("connect_acquires_control", connect_acquires_control),
    ("connect_by_ip_without_discovery", connect_by_ip_without_discovery),
    ("second_connection_is_denied_while_controlled", second_connection_is_denied_while_controlled),
    ("heartbeat_keeps_control", heartbeat_keeps_control),
    ("genicam_xml_is_fetched_and_parsed", genicam_xml_is_fetched_and_parsed),
    ("identity_features_match_discovery", identity_features_match_discovery),
    ("image_format_features_are_readable", image_format_features_are_readable),
    ("packet_size_register_round_trip", packet_size_register_round_trip),
    ("float_feature_write_round_trip", float_feature_write_round_trip),
    ("enum_feature_write_round_trip", enum_feature_write_round_trip),
    ("stream_callback_receives_frames", stream_callback_receives_frames),
    ("stream_channel_zero_copy_frames", stream_channel_zero_copy_frames),
    ("stream_with_custom_packet_size", stream_with_custom_packet_size),
    ("stream_can_restart", stream_can_restart),
    ("chunk_data_matches_genicam_browser", chunk_data_matches_genicam_browser),
];

// ---------------------------------------------------------------------------------------------
// Discovery / control channel
// ---------------------------------------------------------------------------------------------

fn discovery_finds_camera_by_serial(cam: &LiveCam) -> TestResult {
    let info = &cam.info;
    ensure!(!info.manufacturer.trim().is_empty(), "empty manufacturer in discovery ack");
    ensure!(!info.model.trim().is_empty(), "empty model in discovery ack");
    ensure!(info.id.contains(info.serial.trim()), "id {:?} does not contain serial {:?}", info.id, info.serial);
    ensure!(!info.current_ip.is_unspecified(), "camera reported current IP 0.0.0.0");
    Ok(Outcome::Passed)
}

fn connect_acquires_control(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    ensure!(camera.has_control(), "no control-channel privilege right after connect");
    let version = camera.device().read_register(offset::VERSION)?;
    ensure!(version != 0, "GevVersion register reads 0");
    Ok(Outcome::Passed)
}

fn connect_by_ip_without_discovery(cam: &LiveCam) -> TestResult {
    let camera = Camera::connect(cam.info.current_ip)?;
    ensure!(camera.has_control(), "no control-channel privilege right after connect");
    Ok(Outcome::Passed)
}

fn second_connection_is_denied_while_controlled(cam: &LiveCam) -> TestResult {
    let first = Camera::new(&cam.info)?;
    let second = Camera::new(&cam.info);
    ensure!(second.is_err(), "a second client got control while the first still held it exclusively");
    drop(first);
    // Dropping the first `Camera` releases the privilege, so a new client must now get it.
    let third = Camera::new(&cam.info)?;
    ensure!(third.has_control(), "reconnect after releasing control did not get control");
    Ok(Outcome::Passed)
}

fn heartbeat_keeps_control(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    let timeout_ms = camera.device().read_register(offset::HEARTBEAT_TIMEOUT)? as u64;
    let wait = Duration::from_millis((timeout_ms * 3 / 2).clamp(2_000, 6_000));
    std::thread::sleep(wait);
    ensure!(camera.has_control(), "control lost after {wait:?} (heartbeat timeout {timeout_ms} ms)");
    // A register read still succeeding proves the device agrees we are still connected.
    camera.device().read_register(offset::VERSION)?;
    Ok(Outcome::Passed)
}

// ---------------------------------------------------------------------------------------------
// GenICam
// ---------------------------------------------------------------------------------------------

fn genicam_xml_is_fetched_and_parsed(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    let xml = camera.genicam_xml();
    ensure!(xml.contains("RegisterDescription"), "GenICam XML has no RegisterDescription ({} bytes)", xml.len());
    ensure!(!camera.categories()?.is_empty(), "Root category has no children");
    ensure!(camera.feature_kind("Width").is_some(), "no Width feature in the GenICam XML");
    Ok(Outcome::Passed)
}

fn identity_features_match_discovery(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    let checks = [
        ("DeviceSerialNumber", &cam.info.serial),
        ("DeviceModelName", &cam.info.model),
        ("DeviceVendorName", &cam.info.manufacturer),
    ];
    let mut checked = 0;
    for (feature, expected) in checks {
        if camera.feature_kind(feature).is_none() {
            continue;
        }
        let value = camera.read::<String>(feature)?;
        ensure!(
            value.trim() == expected.trim(),
            "{feature} = {value:?}, but discovery reported {expected:?}"
        );
        checked += 1;
    }
    if checked == 0 {
        return Ok(Outcome::Skipped("camera exposes none of the Device*Name/SerialNumber features".into()));
    }
    Ok(Outcome::Passed)
}

fn image_format_features_are_readable(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    let width = camera.read::<i64>("Width")?;
    let height = camera.read::<i64>("Height")?;
    let payload_size = camera.read::<i64>("PayloadSize")?;
    ensure!(width > 0 && height > 0, "non-positive image size {width}x{height}");
    ensure!(payload_size > 0, "non-positive PayloadSize {payload_size}");
    if camera.feature_kind("PixelFormat").is_some() {
        let pixel_format = camera.read::<String>("PixelFormat")?;
        ensure!(!pixel_format.trim().is_empty(), "empty PixelFormat");
    }
    Ok(Outcome::Passed)
}

fn packet_size_register_round_trip(cam: &LiveCam) -> TestResult {
    let camera = Camera::new(&cam.info)?;
    let device = camera.device();
    let original = device.stream_packet_size()?;
    let target = if original == 1400 { 1200 } else { 1400 };
    let result = (|| -> TestResult {
        device.set_stream_packet_size(target)?;
        let read_back = device.stream_packet_size()?;
        ensure!(read_back == target, "GevSCPSPacketSize read back {read_back}, wrote {target}");
        Ok(Outcome::Passed)
    })();
    device.set_stream_packet_size(original)?;
    result
}

fn float_feature_write_round_trip(cam: &LiveCam) -> TestResult {
    const FEATURE: &str = "ExposureTime";
    let camera = Camera::new(&cam.info)?;
    if camera.feature_kind(FEATURE).is_none() {
        return Ok(Outcome::Skipped(format!("no {FEATURE} feature")));
    }
    let original = camera.read::<f64>(FEATURE)?;
    // Moving toward a shorter exposure is the direction least likely to leave the valid range.
    let target = if original > 1_000.0 { original - 500.0 } else { original + 500.0 };
    let result = (|| -> TestResult {
        camera.write(FEATURE, target)?;
        let read_back = camera.read::<f64>(FEATURE)?;
        // Devices may quantize exposure, so require only that it moved most of the way there.
        ensure!(
            (read_back - target).abs() < (original - target).abs() / 2.0,
            "{FEATURE}: wrote {target}, read back {read_back} (was {original})"
        );
        Ok(Outcome::Passed)
    })();
    camera.write(FEATURE, original)?;
    result
}

fn enum_feature_write_round_trip(cam: &LiveCam) -> TestResult {
    const FEATURE: &str = "TestPattern";
    let camera = Camera::new(&cam.info)?;
    if camera.feature_kind(FEATURE) != Some("Enumeration") {
        return Ok(Outcome::Skipped(format!("no {FEATURE} enumeration")));
    }
    let original = camera.read::<String>(FEATURE)?;
    let result = (|| -> TestResult {
        camera.write(FEATURE, "Off".to_string())?;
        let read_back = camera.read::<String>(FEATURE)?;
        ensure!(read_back == "Off", "{FEATURE}: wrote \"Off\", read back {read_back:?}");
        Ok(Outcome::Passed)
    })();
    camera.write(FEATURE, original.clone())?;
    let restored = camera.read::<String>(FEATURE)?;
    ensure!(restored == original, "{FEATURE} not restored: {restored:?} != {original:?}");
    result
}

// ---------------------------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------------------------

/// Connect with any acquisition left running by an earlier (possibly failed) run stopped first.
fn connect_for_streaming(cam: &LiveCam) -> Result<Camera> {
    let camera = Camera::new(&cam.info)?;
    let _ = camera.execute_command("AcquisitionStop");
    Ok(camera)
}

/// Check a batch of frames: every one complete, within `PayloadSize`, and in frame-id order.
fn check_frames(frames: &[Buffer], payload_size: i64) -> std::result::Result<(), String> {
    let mut previous_id = None;
    for (i, frame) in frames.iter().enumerate() {
        if frame.status != BufferStatus::Success {
            return Err(format!("frame {i} (id {}) has status {:?}", frame.frame_id, frame.status));
        }
        let len = frame.data().len() as i64;
        if len == 0 || len > payload_size {
            return Err(format!("frame {i} (id {}) has {len} bytes, PayloadSize is {payload_size}", frame.frame_id));
        }
        if let Some(prev) = previous_id {
            if frame.frame_id <= prev {
                return Err(format!("frame {i} id {} does not follow previous id {prev}", frame.frame_id));
            }
        }
        previous_id = Some(frame.frame_id);
    }
    Ok(())
}

/// Start a callback stream on `camera`, collect `n` frames, and stop it again. Also returns the
/// `PayloadSize` in effect while streaming: it can depend on the packet size the stream programs
/// (it does on the C6-2040-GigE), so it must be read after the stream has started.
fn stream_frames(camera: &Camera, cfg: Option<StreamConfig>, n: usize) -> std::result::Result<(Vec<Buffer>, i64), Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel();
    let callback = move |buffer: Buffer| {
        let _ = tx.send(buffer);
    };
    let stream = match cfg {
        Some(cfg) => camera.start_stream_with_config(cfg, callback)?,
        None => camera.start_stream(callback)?,
    };
    let payload_size = camera.read::<i64>("PayloadSize");
    let mut frames = Vec::with_capacity(n);
    for _ in 0..n {
        match rx.recv_timeout(FRAME_TIMEOUT) {
            Ok(buffer) => frames.push(buffer),
            Err(_) => break,
        }
    }
    camera.stop_stream(stream)?;
    if frames.len() < n {
        return Err(format!("received only {}/{n} frames ({FRAME_TIMEOUT:?} timeout per frame)", frames.len()).into());
    }
    Ok((frames, payload_size?))
}

fn stream_callback_receives_frames(cam: &LiveCam) -> TestResult {
    let camera = connect_for_streaming(cam)?;
    let (frames, payload_size) = stream_frames(&camera, None, 10)?;
    check_frames(&frames, payload_size)?;
    Ok(Outcome::Passed)
}

fn stream_channel_zero_copy_frames(cam: &LiveCam) -> TestResult {
    let camera = connect_for_streaming(cam)?;
    let width = camera.read::<i64>("Width")?;
    let height = camera.read::<i64>("Height")?;

    let (stream, pool) = camera.start_stream_channel()?;
    let result = (|| -> TestResult {
        let payload_size = camera.read::<i64>("PayloadSize")?;
        for i in 0..5 {
            let buffer = pool
                .timeout_pop_buffer(FRAME_TIMEOUT)
                .ok_or_else(|| format!("frame {i}: no buffer within {FRAME_TIMEOUT:?}"))?;
            let checked = check_frames(std::slice::from_ref(&buffer), payload_size);
            let image = buffer.image;
            let payload_type = buffer.payload_type;
            pool.push_buffer(buffer);
            checked?;
            if payload_type == PayloadType::Image {
                let image = image.ok_or_else(|| format!("frame {i}: image payload without image info"))?;
                ensure!(
                    image.width as i64 == width && image.height as i64 == height,
                    "frame {i}: leader says {}x{}, features say {width}x{height}",
                    image.width,
                    image.height
                );
            }
        }
        Ok(Outcome::Passed)
    })();
    camera.stop_stream(stream)?;
    result
}

fn stream_with_custom_packet_size(cam: &LiveCam) -> TestResult {
    let camera = connect_for_streaming(cam)?;
    let cfg = StreamConfig {
        packet_size: 1000,
        ..StreamConfig::default()
    };
    let (frames, payload_size) = stream_frames(&camera, Some(cfg), 5)?;
    check_frames(&frames, payload_size)?;
    Ok(Outcome::Passed)
}

fn stream_can_restart(cam: &LiveCam) -> TestResult {
    let camera = connect_for_streaming(cam)?;
    for round in 1..=2 {
        let (frames, payload_size) = stream_frames(&camera, None, 3).map_err(|e| format!("round {round}: {e}"))?;
        check_frames(&frames, payload_size).map_err(|e| format!("round {round}: {e}"))?;
    }
    Ok(Outcome::Passed)
}

// ---------------------------------------------------------------------------------------------
// Chunk data
// ---------------------------------------------------------------------------------------------

/// The per-frame chunk features the camera's GenICam browser (CVB GenICamBrowser, "Chunk Data
/// Control") lists with values for the default scan line, in its order.
const SCAN_LINE_CHUNK_FEATURES: &[&str] = &[
    "ChunkTimestamp",
    "ChunkFrameID",
    "ChunkEncoderValue",
    "ChunkAO0",
    "ChunkAI0",
    "ChunkTriggerTimestamp",
    "ChunkTriggerEncoderValue",
    "ChunkFlags",
    "ChunkLineStatusAll",
];

/// Values the reference browser showed that don't change from frame to frame on this setup:
/// no encoder, analog I/O or flags in use, and a fixed digital-line state (`ChunkLineStatusAll`
/// = 28 depends on how the camera's I/O lines are wired).
const STATIC_CHUNK_VALUES: &[(&str, i64)] = &[
    ("ChunkEncoderValue", 0),
    ("ChunkAO0", 0),
    ("ChunkAI0", 0),
    ("ChunkTriggerEncoderValue", 0),
    ("ChunkFlags", 0),
    ("ChunkLineStatusAll", 28),
];

/// Features the browser shows as "-" in area-scan mode: gated on 3D mode (`is3dModeActive`), and
/// the region values additionally need a region chunk the camera only sends in 3D mode.
const THREE_D_ONLY_CHUNK_FEATURES: &[&str] = &[
    "ChunkFirstProfile",
    "ChunkYValidProfiles",
    "ChunkScanLineSelector",
    "ChunkRegionSelector",
];
const REGION_CHUNK_FEATURES: &[&str] = &[
    "ChunkRegionIDValue",
    "ChunkRegionOffsetY",
    "ChunkRegionHeight",
    "ChunkRegionOffsetX",
    "ChunkRegionWidth",
    "ChunkRegionRangeNumValid",
    "ChunkRegionRangeMax",
    "ChunkRegionRangeSum",
    "ChunkRegionRangeMin",
];

fn chunk_data_matches_genicam_browser(cam: &LiveCam) -> TestResult {
    let camera = connect_for_streaming(cam)?;
    for feature in ["ChunkModeActive", "ChunkFrameID", "DeviceScanType"] {
        if camera.feature_kind(feature).is_none() {
            return Ok(Outcome::Skipped(format!("no {feature} feature")));
        }
    }
    let scan_type = camera.read::<String>("DeviceScanType")?;
    if scan_type != "Areascan" {
        return Ok(Outcome::Skipped(format!(
            "reference values were taken in Areascan mode, camera is in {scan_type}"
        )));
    }

    let original_chunk_mode = camera.read::<bool>("ChunkModeActive")?;
    let result = (|| -> TestResult {
        camera.write("ChunkModeActive", true)?;
        ensure!(camera.read::<bool>("ChunkModeActive")?, "ChunkModeActive did not turn on");

        for &feature in THREE_D_ONLY_CHUNK_FEATURES {
            ensure!(!camera.is_available(feature)?, "{feature} is available in Areascan mode");
        }
        for &feature in SCAN_LINE_CHUNK_FEATURES {
            ensure!(camera.is_available(feature)?, "{feature} is not available");
        }
        let tick_frequency = camera.read::<i64>("GevTimestampTickFrequency").unwrap_or(1_000_000_000);

        let (stream, pool) = camera.start_stream_channel()?;
        let payload_size = camera.read::<i64>("PayloadSize");
        let frames = (|| -> std::result::Result<Vec<Buffer>, Box<dyn std::error::Error>> {
            let mut frames = Vec::new();
            for i in 0..5 {
                let buffer = pool
                    .timeout_pop_buffer(FRAME_TIMEOUT)
                    .ok_or_else(|| format!("frame {i}: no buffer within {FRAME_TIMEOUT:?}"))?;
                frames.push(buffer.clone());
                pool.push_buffer(buffer);
            }
            Ok(frames)
        })();
        camera.stop_stream(stream)?;
        let frames = frames?;
        check_frames(&frames, payload_size?)?;

        let mut first: Option<(u64, i64)> = None;
        let mut shown = String::new();
        for frame in &frames {
            let values = SCAN_LINE_CHUNK_FEATURES
                .iter()
                .map(|&f| camera.read_chunk::<i64>(frame, f).map(|v| (f, v)))
                .collect::<Result<Vec<_>>>()?;
            let value = |name: &str| values.iter().find(|(f, _)| *f == name).map(|&(_, v)| v).unwrap();

            // The chunk timestamp is the one the GVSP leader carries.
            ensure!(
                value("ChunkTimestamp") as u64 == frame.timestamp_ns,
                "frame {}: ChunkTimestamp {} != leader timestamp {}",
                frame.frame_id,
                value("ChunkTimestamp"),
                frame.timestamp_ns
            );
            let trigger_delta = (value("ChunkTriggerTimestamp") - value("ChunkTimestamp")).abs();
            ensure!(
                value("ChunkTriggerTimestamp") > 0 && trigger_delta < tick_frequency,
                "frame {}: ChunkTriggerTimestamp {} is not within a second of ChunkTimestamp {}",
                frame.frame_id,
                value("ChunkTriggerTimestamp"),
                value("ChunkTimestamp")
            );
            // The camera's frame counter advances with the stream's block ids.
            let (first_block, first_counter) = *first.get_or_insert((frame.frame_id, value("ChunkFrameID")));
            ensure!(
                value("ChunkFrameID") - first_counter == (frame.frame_id - first_block) as i64,
                "frame {}: ChunkFrameID {} does not advance with the block id (first: block {first_block}, ChunkFrameID {first_counter})",
                frame.frame_id,
                value("ChunkFrameID")
            );
            for &(feature, expected) in STATIC_CHUNK_VALUES {
                ensure!(
                    value(feature) == expected,
                    "frame {}: {feature} = {}, reference shows {expected}",
                    frame.frame_id,
                    value(feature)
                );
            }
            // No region chunk in Areascan mode, so there is nothing for these to read ("-").
            for &feature in REGION_CHUNK_FEATURES {
                ensure!(
                    camera.read_chunk::<i64>(frame, feature).is_err(),
                    "frame {}: {feature} unexpectedly has a value",
                    frame.frame_id
                );
            }
            shown = values.iter().map(|(f, v)| format!("{f}={v}")).collect::<Vec<_>>().join(" ");
        }
        Ok(Outcome::PassedWith(format!("last frame: {shown}")))
    })();
    camera.write("ChunkModeActive", original_chunk_mode)?;
    result
}

// ---------------------------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Args {
    serial: Option<String>,
    bind: Option<Ipv4Addr>,
    broadcast: Option<Ipv4Addr>,
    filters: Vec<String>,
    list: bool,
}

/// libtest flags that cargo or an IDE may pass and that take a separate value; the value must be
/// consumed so it isn't taken for a name filter.
const IGNORED_FLAGS_WITH_VALUE: &[&str] = &["--test-threads", "--color", "--format", "--logfile", "--skip", "-Z"];

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with('-') => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline_value
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--serial" => args.serial = Some(value("--serial")?),
            "--bind" => args.bind = Some(value("--bind")?.parse().map_err(|e| format!("--bind: {e}"))?),
            "--broadcast" => {
                args.broadcast = Some(value("--broadcast")?.parse().map_err(|e| format!("--broadcast: {e}"))?)
            }
            "--list" => args.list = true,
            f if IGNORED_FLAGS_WITH_VALUE.contains(&f) => {
                if inline_value.is_none() {
                    it.next();
                }
            }
            f if f.starts_with('-') => {}
            _ => args.filters.push(arg),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let _ = env_logger::try_init();
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{GROUP}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let selected: Vec<(String, TestFn)> = TESTS
        .iter()
        .map(|(name, f)| (format!("{GROUP}::{name}"), *f))
        .filter(|(name, _)| args.filters.is_empty() || args.filters.iter().any(|f| name.contains(f.as_str())))
        .collect();

    if args.list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }

    let Some(serial) = args.serial else {
        println!("{GROUP}: skipped (pass --serial <SN> to run against a live camera)");
        return ExitCode::SUCCESS;
    };

    let opts = DiscoveryOptions {
        bind_addrs: vec![(args.bind.unwrap_or(Ipv4Addr::UNSPECIFIED), args.broadcast)],
        timeout: DISCOVERY_TIMEOUT,
    };
    let found = match discover(&opts) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("{GROUP}: discovery failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(info) = found.iter().find(|d| d.serial.trim() == serial.trim()).cloned() else {
        let serials: Vec<&str> = found.iter().map(|d| d.serial.trim()).collect();
        eprintln!("{GROUP}: no camera with serial {serial:?} found (discovered serials: {serials:?})");
        return ExitCode::FAILURE;
    };
    println!("{GROUP}: using {} at {}", info.id, info.current_ip);
    let cam = LiveCam { info };

    println!("\nrunning {} tests", selected.len());
    let (mut passed, mut failed, mut ignored) = (0, Vec::new(), 0);
    for (name, test) in &selected {
        match catch_unwind(AssertUnwindSafe(|| test(&cam))) {
            Ok(Ok(Outcome::Passed)) => {
                println!("test {name} ... ok");
                passed += 1;
            }
            Ok(Ok(Outcome::PassedWith(details))) => {
                println!("test {name} ... ok\n    {details}");
                passed += 1;
            }
            Ok(Ok(Outcome::Skipped(reason))) => {
                println!("test {name} ... ignored, {reason}");
                ignored += 1;
            }
            Ok(Err(e)) => {
                println!("test {name} ... FAILED\n    {e}");
                failed.push(name.clone());
            }
            Err(_) => {
                println!("test {name} ... FAILED (panicked)");
                failed.push(name.clone());
            }
        }
    }

    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {passed} passed; {} failed; {ignored} ignored; {} filtered out\n",
        failed.len(),
        TESTS.len() - selected.len()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

//! Phase-5 live-hardware check: the umbrella `Camera`/`discover()`/`start_stream` API end-to-end
//! against the real camera — the spec's example program (`ai/rust-port.md`), adapted to print
//! progress and stop after a handful of frames instead of running forever.
//!
//! `cargo run -p aravis-port --example live_check_umbrella -- 169.254.1.1 169.254.255.255`

use aravis_port::prelude::*;
use std::time::Duration;

fn main() -> Result<()> {
    env_logger::init();
    let mut args = std::env::args().skip(1);
    let bind_ip: std::net::Ipv4Addr = args
        .next()
        .expect("usage: live_check_umbrella <bind-ip> <directed-broadcast-ip>")
        .parse()
        .unwrap();
    let broadcast_ip: std::net::Ipv4Addr = args
        .next()
        .expect("usage: live_check_umbrella <bind-ip> <directed-broadcast-ip>")
        .parse()
        .unwrap();

    // Discover cameras on the network (mirrors `aravis_port::discover`, but with an explicit
    // bind/broadcast pair since std has no safe interface-enumeration API).
    let cameras = aravis_port::net::discover(&aravis_port::net::DiscoveryOptions {
        bind_addrs: vec![(bind_ip, Some(broadcast_ip))],
        timeout: Duration::from_secs(2),
    })?;
    println!("discovered {} camera(s)", cameras.len());
    let info = cameras.first().expect("no cameras found");
    println!("connecting to {} ({})", info.id, info.current_ip);

    let camera = Camera::new(info)?;

    let exposure = camera.read::<f64>("ExposureTime")?;
    println!("ExposureTime = {exposure}");
    println!("categories = {:?}", camera.categories()?);

    let mut count = 0;
    let stream = camera.start_stream(move |buffer| {
        count += 1;
        println!(
            "[{count}] Received frame id {} size {} status {:?}",
            buffer.frame_id,
            buffer.data().len(),
            buffer.status
        );
    })?;

    std::thread::sleep(Duration::from_secs(3));
    camera.stop_stream(stream)?;
    println!("done");
    Ok(())
}

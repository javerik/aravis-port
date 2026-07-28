//! Manual discovery smoke test: `cargo run -p aravis-port-device --example discover -- 169.254.1.1 169.254.255.255`

use std::net::Ipv4Addr;
use std::time::Duration;

use aravis_port_device::net::{discover, DiscoveryOptions};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bind_addr: Ipv4Addr = args
        .get(1)
        .map(|s| s.parse().expect("invalid bind address"))
        .unwrap_or(Ipv4Addr::UNSPECIFIED);
    let directed_broadcast: Option<Ipv4Addr> = args.get(2).map(|s| s.parse().expect("invalid broadcast address"));

    let opts = DiscoveryOptions {
        bind_addrs: vec![(bind_addr, directed_broadcast)],
        timeout: Duration::from_millis(1500),
    };

    let devices = discover(&opts).expect("discovery failed");
    println!("found {} device(s):", devices.len());
    for d in devices {
        println!(
            "  id={} manufacturer={:?} model={:?} serial={:?} version={:?} mac={} ip={}",
            d.id, d.manufacturer, d.model, d.serial, d.version, d.mac, d.current_ip
        );
    }
}

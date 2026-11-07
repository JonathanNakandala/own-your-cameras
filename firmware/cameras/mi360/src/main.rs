pub mod board;
mod capture;
mod config;
pub mod graph_settings;
pub mod h264;
pub mod media;
pub mod motion;
pub mod motor_clock;
mod onvif;
pub mod pointing;
mod presets;
mod probe;
mod runtime;
mod sdk;
mod selftest;
mod setting_service;
mod settings;
pub mod watchdog;

fn main() -> std::io::Result<()> {
    let command = std::env::args().skip(1).collect::<Vec<_>>();
    match command.as_slice() {
        [cmd] if cmd == "selftest" => selftest::run(),
        [cmd] if cmd == "inspect" => {
            for path in ["/proc/jz/sinfo/info", "/proc/jz/motor/motor_info"] {
                println!("{path}:\n{}", std::fs::read_to_string(path)?);
            }
            Ok(())
        }
        [cmd] if cmd == "sdk-version" => {
            println!("{}", sdk::version()?);
            Ok(())
        }
        [cmd, output, frames] if cmd == "capture" => {
            let frames = frames
                .parse()
                .map_err(|_| std::io::Error::other("invalid frame count"))?;
            capture::run(output, frames)
        }
        [cmd, prefix] if cmd == "media-probe" => probe::run(prefix),
        [cmd, prefix] if cmd == "controls-probe" => probe::controls(prefix),
        [cmd, cases] if cmd == "video-spec-probe" => probe::video_specs(cases),
        [cmd, flag, path] if cmd == "serve" && flag == "--config" => runtime::run(path),
        [cmd, flag, path] if cmd == "check-config" && flag == "--config" => {
            let configuration = config::load(path)?;
            if !configuration.runtime.watchdog {
                return Err(std::io::Error::other(
                    "production configuration requires watchdog",
                ));
            }
            #[cfg(all(target_os = "linux", target_arch = "mips"))]
            sdk::verify_library()?;
            println!("audio={}", configuration.runtime.audio);
            println!("watchdog=true");
            println!("cold_controls={}", configuration.runtime.cold_controls);
            Ok(())
        }
        [cmd] if cmd == "watchdog-probe" => {
            let mut owner = watchdog::Watchdog::open(40).map_err(std::io::Error::other)?;
            for _ in 0..3 {
                owner.feed()?;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            let timeout = owner.timeout();
            owner.disarm()?;
            println!(
                "native watchdog fed, read back {} seconds and explicitly disarmed",
                timeout.requested_seconds
            );
            Ok(())
        }
        [cmd, address] if cmd == "tcp-probe" => {
            use std::net::ToSocketAddrs;
            let address = address
                .to_socket_addrs()?
                .find(std::net::SocketAddr::is_ipv4)
                .ok_or_else(|| std::io::Error::other("no IPv4 endpoint"))?;
            let stream =
                std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(3))?;
            println!("TCP connected to {}", stream.peer_addr()?);
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: cam selftest | inspect | sdk-version | capture /tmp/mi360-OUTPUT FRAMES | media-probe /tmp/mi360-PREFIX | controls-probe PREFIX | video-spec-probe CASE | watchdog-probe | tcp-probe HOST:PORT | check-config --config PATH | serve --config PATH"
            );
            std::process::exit(2);
        }
    }
}

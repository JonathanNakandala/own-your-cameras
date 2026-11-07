//! Minimal MQTT 3.1.1 (QoS 0 publish, QoS 0 subscribe, keep-alive) on a blocking `TcpStream`. The
//! application reconnects; this module owns one connection. Small on purpose: the DS-KD8003-IME1 has
//! 184 MiB of RAM but under 1 MiB of flash for the whole application.
pub mod client;
pub mod packet;

pub use client::{Client, Incoming, Options};

//! In-process RTSP server: `main` / `sub` as H.264 over
//! RTP, interleaved on the RTSP TCP connection or over UDP, with digest authentication.

mod agc;
pub mod audio;
pub mod hub;
pub mod md5;
pub mod proto;
mod rtp;
pub mod server;
pub mod talk;

mod socket_limits;

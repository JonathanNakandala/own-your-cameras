//! Hardware-independent parts of the DS-KD8003-IME1 native application: configuration, PIN access
//! policy, keypad frame shapes, Home Assistant MQTT discovery and a small MQTT 3.1.1 client.
//! Everything that touches the board lives in the binary and is compiled for the ARM target only.
pub mod access;
pub mod config;
pub mod discovery;
pub mod keypad_frames;
pub mod mqtt;
pub mod pin;

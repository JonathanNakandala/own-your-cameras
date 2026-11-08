//! Hardware-independent parts of the DS-KD8003-IME1 native application: configuration, PIN access
//! policy, keypad frame shapes, the input edge source, the gate's entities on the shared MQTT layer,
//! its live settings with their apply owner, and its telemetry.
//! Everything that touches the board lives in the binary and is compiled for the ARM target only.
pub mod access;
pub mod clip;
pub mod config;
pub mod home;
pub mod inputs;
pub mod keypad_frames;
pub mod media;
pub mod pin;
pub mod settings;
pub mod telemetry;

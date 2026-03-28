//! Library entrypoint for the motion detection CLI.
//!
//! The binary in `src/main.rs` only forwards to `run()` so the rest of the code can be
//! organized into small modules and exercised by unit tests.

pub mod app;
pub mod config;
pub mod error;
pub mod ffmpeg;
pub mod motion;
pub mod mqtt;

pub use app::run;

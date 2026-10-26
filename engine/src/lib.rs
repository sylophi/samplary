//! The engine behind Samplary: the sample library, its analysis and its
//! search, served to the app over stdin and stdout.

// Written before these lints, and kept as they are.
#[allow(
    clippy::eq_op,
    clippy::needless_range_loop,
    clippy::chunks_exact_to_as_chunks
)]
pub mod analysis;
pub mod clap;
pub mod classify;
pub mod db;
pub mod decode;
pub mod dsp;
pub mod engine;
pub mod index;
pub mod library;
pub mod names;
pub mod pitch;
pub mod scan;
pub mod server;
pub mod watch;
#[allow(clippy::chunks_exact_to_as_chunks)]
pub mod waveform;

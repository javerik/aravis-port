//! Minimal pure-Rust ZIP + DEFLATE support, needed because real GenICam cameras (confirmed
//! against the live C5-2040-GigE) serve their XML inside a `.zip` file, and no zip/flate2 crate
//! is in this project's allowed dependency list.

mod archive;
mod deflate;

pub use archive::extract_first_file;

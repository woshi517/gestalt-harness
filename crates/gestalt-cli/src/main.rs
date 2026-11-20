#![forbid(unsafe_code)]
//! Thin CLI entrypoint for the Gestalt agent harness.

fn main() {
    println!("gestalt {}", env!("CARGO_PKG_VERSION"));
}

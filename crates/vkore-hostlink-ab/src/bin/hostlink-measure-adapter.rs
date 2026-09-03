//! Platform entry point for the HostLink A/B measurement adapter.

#[cfg(unix)]
include!("support/hostlink_measure_adapter_unix.rs");

#[cfg(not(unix))]
fn main() {
    eprintln!("hostlink-measure-adapter: measurement execution requires Unix");
    std::process::exit(2);
}

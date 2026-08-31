//! Platform entry point for HostLink socket conformance tests.

#[cfg(unix)]
include!("support/conformance_unix.rs");

#[cfg(not(unix))]
#[test]
fn unix_socket_conformance_is_not_available_on_this_host() {
    // TCP coverage remains in the daemon unit tests. The extracted
    // conformance suite exercises UnixListener/UnixStream explicitly.
}

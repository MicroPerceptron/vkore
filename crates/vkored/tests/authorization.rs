//! Private helper entry points must refuse ordinary unauthenticated invocation.
use std::process::Command;

#[test]
fn memory_helper_refuses_unprivileged_or_unidentified_callers_and_bad_requests() {
    let before = rustix::process::getrlimit(rustix::process::Resource::Memlock);
    for arguments in [
        vec![],
        vec!["0"],
        vec!["2048"],
        vec!["1048577"],
        vec!["2048", "--pid", "1"],
        vec!["NaN"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_vkore"))
            .arg("--vkore-memory-helper")
            .args(arguments)
            .env_remove("PKEXEC_UID")
            .output()
            .unwrap();
        assert!(!result.status.success());
    }
    let after = rustix::process::getrlimit(rustix::process::Resource::Memlock);
    assert_eq!(before.current, after.current);
    assert_eq!(before.maximum, after.maximum);
}

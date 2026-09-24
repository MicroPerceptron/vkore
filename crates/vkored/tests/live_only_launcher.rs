//! Production entrypoints must never fall back to test fixtures.

#[test]
fn harness_requires_a_real_guest_without_an_opt_in_flag() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vkore-harnessd"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--qemu is required"));
}

#[test]
fn model_arguments_cannot_start_a_simulated_guest() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vkore-harnessd"))
        .args([
            "--llama-server",
            "/not/a/server",
            "--llama-model",
            "/not/a/model",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires real Kore"));
}

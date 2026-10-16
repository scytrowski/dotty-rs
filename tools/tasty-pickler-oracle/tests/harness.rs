use std::process::Command;

#[test]
fn environment_independent_harness_failure_controls() {
    let output = Command::new("python3")
        .arg("test_run.py")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Python 3 is required by repository validation tooling");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

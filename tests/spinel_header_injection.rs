//! The spinel lane's HTTP servers drop a header that carries a control
//! character instead of writing it, so a CRLF in an app-supplied value —
//! a redirect Location built from a param, a Content-Disposition, a
//! cookie option — cannot end the header and start one of its own
//! (`Set-Cookie: pwned=1`) or a body. That is Puma's rule, the server a
//! Rails app sits behind: a key or value holding a control character
//! (tab excepted, in a value) is dropped and the rest of the response
//! goes out.
//!
//! All three servers (threaded — the default —, scheduled, blocking) are
//! checked, under CRuby, by the driver `tests/spinel_header_injection.rb`
//! over the scripted socket in `tests/tep_server_harness.rb`.

use std::path::Path;
use std::process::Command;

const CHECKS: usize = 15;

#[test]
fn a_header_carrying_a_control_character_never_reaches_the_wire() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("ruby")
        .arg(root.join("tests/spinel_header_injection.rb"))
        .output()
        .expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // `done` is the last line; its absence means the driver died part
    // way, which an absence of FAIL lines alone would read as a pass.
    assert!(
        stdout.lines().any(|l| l == "done"),
        "driver did not finish\n=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}"
    );
    let failed: Vec<&str> = stdout.lines().filter(|l| l.starts_with("FAIL")).collect();
    assert!(failed.is_empty(), "{}\n=== stdout ===\n{stdout}", failed.join("\n"));
    assert!(
        stdout.lines().any(|l| l == format!("{CHECKS}/{CHECKS} checks pass")),
        "fewer checks ran than the driver makes\n=== stdout ===\n{stdout}"
    );
}

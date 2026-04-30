//! Golden-fixture coverage for `redo::cli::diff::render_unified`.
//!
//! Builds two known canonical-line sequences, renders the no-color unified
//! diff into a buffer, and compares to a checked-in expected output. If you
//! tweak the canonical-line format intentionally, regenerate the fixture.

use std::path::PathBuf;

use redo::cli::diff;

#[test]
fn unified_diff_matches_golden_fixture() {
    let a: Vec<String> = vec![
        "#    0 Marker hook:PreToolUse [Bash]".into(),
        "#    1 Output 5 bytes".into(),
        "#    2 Marker hook:PostToolUse".into(),
        "#    3 Marker session_end".into(),
    ];
    let b: Vec<String> = vec![
        "#    0 Marker hook:PreToolUse [Bash]".into(),
        "#    1 Output 8 bytes".into(),
        "#    2 Marker hook:PostToolUse".into(),
        "#    3 Marker session_end_v2".into(),
    ];

    let mut buf: Vec<u8> = Vec::new();
    diff::render_unified(&mut buf, &a, &b, &"session-a", &"session-b", 3, false).expect("render");
    let actual = String::from_utf8(buf).expect("utf8");

    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "diff",
        "expected.txt",
    ]
    .iter()
    .collect();
    let expected =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));

    assert_eq!(
        actual, expected,
        "diff output mismatch\n--- actual ---\n{actual}\n--- expected ---\n{expected}"
    );
}

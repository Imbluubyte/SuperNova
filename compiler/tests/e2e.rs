//! End-to-end tests: compile and run every tests/cases/*.sn and compare stdout
//! with the matching .out file; compile every tests/errors/*.sn and check that
//! compilation fails with a message containing the text in the matching .err.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("tests")
}

fn snc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_snc"))
}

fn files(dir: &str, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().map_or(false, |e| e == ext))
        .collect();
    v.sort();
    v
}

#[test]
fn programs_produce_expected_output() {
    let cases = files("cases", "sn");
    assert!(!cases.is_empty());
    for sn in cases {
        let expected = std::fs::read_to_string(sn.with_extension("out")).unwrap();
        let out = snc().arg("run").arg(&sn).output().unwrap();
        assert!(
            out.status.success(),
            "{} failed:\n{}",
            sn.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "output mismatch for {}",
            sn.display()
        );
    }
}

#[test]
fn invalid_programs_are_rejected_with_a_clear_error() {
    let cases = files("errors", "sn");
    assert!(!cases.is_empty());
    for sn in cases {
        let want = std::fs::read_to_string(sn.with_extension("err")).unwrap();
        let out = snc().arg("check").arg(&sn).output().unwrap();
        assert!(!out.status.success(), "{} should not compile", sn.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(want.trim()),
            "{}: expected error containing {:?}, got:\n{}",
            sn.display(),
            want.trim(),
            stderr
        );
    }
}

#[test]
fn runtime_errors_report_location_and_exit_nonzero() {
    let dir = std::env::temp_dir().join("snc-rt-err-test");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("div.sn");
    std::fs::write(&f, "var a = 10\nvar b = 0\nprint(a // b)\n").unwrap();
    let out = snc().arg("run").arg(&f).output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("division by zero"), "{}", stderr);
    assert!(stderr.contains("div.sn:3:"), "{}", stderr);
}

#[test]
fn runtime_errors_in_collections_are_clear() {
    let cases = files("runtime_errors", "sn");
    assert!(!cases.is_empty());
    for sn in cases {
        let want = std::fs::read_to_string(sn.with_extension("err")).unwrap();
        let out = snc().arg("run").arg(&sn).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{} should fail at run time", sn.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(want.trim()),
            "{}: expected {:?}, got:\n{}",
            sn.display(),
            want.trim(),
            stderr
        );
        assert!(stderr.contains(".sn:"), "{} lacks a source location:\n{}", sn.display(), stderr);
    }
}

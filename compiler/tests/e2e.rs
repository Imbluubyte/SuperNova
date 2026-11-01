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

/// Collecting garbage before *every* allocation flushes out any value the
/// collector fails to see (a missing root would show up as corrupted output
/// or a crash). Programs named slow_* are too allocation-heavy for this.
#[test]
fn programs_survive_a_collection_on_every_allocation() {
    for sn in files("cases", "sn") {
        if sn.file_name().unwrap().to_string_lossy().starts_with("slow_") {
            continue;
        }
        let expected = std::fs::read_to_string(sn.with_extension("out")).unwrap();
        let out = snc().env("SN_GC_STRESS", "1").arg("run").arg(&sn).output().unwrap();
        assert!(
            out.status.success(),
            "{} failed under GC stress:\n{}",
            sn.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            expected,
            "output mismatch under GC stress for {}",
            sn.display()
        );
    }
}

#[test]
fn garbage_is_reclaimed_and_the_heap_stays_small() {
    // slow_gc_churn.sn allocates roughly a gigabyte of short-lived lists in total.
    let sn = root().join("cases").join("slow_gc_churn.sn");
    let out = snc().env("SN_GC_STATS", "1").arg("run").arg(&sn).output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let peak: f64 = stderr
        .split("peak_heap_mb=")
        .nth(1)
        .unwrap_or_else(|| panic!("no GC stats in:\n{}", stderr))
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(peak < 64.0, "peak heap was {} MB; garbage is not being reclaimed", peak);
}

#[test]
fn infinite_recursion_reports_a_stack_overflow() {
    let dir = std::env::temp_dir().join("snc-so-test");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("overflow.sn");
    std::fs::write(
        &f,
        "fn deep(n: Int) -> Int:\n    var xs = [n]\n    return deep(n + 1) + xs[1]\n\nprint(\"before\")\nprint(deep(1))\n",
    )
    .unwrap();
    let out = snc().arg("run").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "before\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("stack overflow"));
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

/// Every ```sn example in docs/CHEATSHEET.md must compile and run, so the
/// cheat sheet cannot drift away from the implementation.
#[test]
fn cheat_sheet_examples_compile_and_run() {
    let md = std::fs::read_to_string(root().join("..").join("docs").join("CHEATSHEET.md")).unwrap();
    let mut program = String::new();
    let mut blocks = 0;
    let mut rest = md.as_str();
    while let Some(start) = rest.find("```sn\n") {
        let body = &rest[start + 6..];
        let end = body.find("```").expect("unterminated code block");
        program.push_str(&body[..end]);
        program.push('\n');
        blocks += 1;
        rest = &body[end + 3..];
    }
    assert!(blocks >= 8, "found only {} example blocks", blocks);
    let dir = std::env::temp_dir().join("snc-cheatsheet-test");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("cheatsheet.sn");
    std::fs::write(&f, program).unwrap();
    let out = snc().arg("run").arg(&f).output().unwrap();
    assert!(out.status.success(), "cheat sheet examples failed:\n{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn multi_file_projects_run_with_expected_output() {
    let base = root().join("projects");
    let mut n = 0;
    for entry in std::fs::read_dir(&base).unwrap() {
        let dir = entry.unwrap().path();
        let main = dir.join("main.sn");
        if !main.is_file() {
            continue;
        }
        n += 1;
        let want = std::fs::read_to_string(dir.join("expected.out")).unwrap();
        let out = snc().arg("run").arg(&main).output().unwrap();
        assert!(out.status.success(), "{}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{}", dir.display());
        // Every collection must also be safe: run with a collection on each allocation.
        let bin = std::env::temp_dir().join(format!("snc-proj-{}", dir.file_name().unwrap().to_string_lossy()));
        let b = snc().arg("build").arg(&main).arg("-o").arg(&bin).output().unwrap();
        assert!(b.status.success());
        let st = Command::new(&bin).env("SN_GC_STRESS", "1").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&st.stdout), want, "{} (GC stress)", dir.display());
    }
    assert!(n > 0);
}

#[test]
fn module_errors_are_rejected_with_a_clear_error() {
    let base = root().join("projects").join("errors");
    let mut n = 0;
    for entry in std::fs::read_dir(&base).unwrap() {
        let dir = entry.unwrap().path();
        let want = std::fs::read_to_string(dir.join("expected.err")).unwrap();
        let out = snc().arg("check").arg(dir.join("main.sn")).output().unwrap();
        assert!(!out.status.success(), "{} should not compile", dir.display());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(want.trim()),
            "{}: expected error containing {:?}, got:\n{}",
            dir.display(),
            want.trim(),
            stderr
        );
        n += 1;
    }
    assert!(n >= 10);
}

#[test]
fn stack_overflow_inside_a_task_is_reported() {
    let dir = std::env::temp_dir().join("snc-task-so-test");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("task_overflow.sn");
    std::fs::write(
        &f,
        "fn deep(n: Int) -> Int:\n    var xs = [n]\n    return deep(n + 1) + xs[1]\n\nasync fn job() -> Int:\n    return deep(1)\n\nprint(\"before\")\nprint(await job())\n",
    )
    .unwrap();
    let out = snc().arg("run").arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "before\n");
    assert!(String::from_utf8_lossy(&out.stderr).contains("stack overflow"));
}

#[test]
fn library_builds_can_be_driven_by_a_c_host() {
    let dir = std::env::temp_dir().join(format!("sn-embed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = root().join("embed");
    let lib = dir.join("libgame.so");
    let b = snc().arg("build").arg(src.join("game.sn")).arg("--lib").arg("-o").arg(&lib).output().unwrap();
    assert!(b.status.success(), "lib build failed:\n{}", String::from_utf8_lossy(&b.stderr));
    assert!(dir.join("libgame.h").exists(), "header was not written");
    let host = dir.join("host");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let c = Command::new(&cc).arg(src.join("host.c")).arg("-o").arg(&host).args(["-ldl", "-lpthread"]).output().unwrap();
    assert!(c.status.success(), "host build failed:\n{}", String::from_utf8_lossy(&c.stderr));
    let expected = std::fs::read_to_string(src.join("expected.out")).unwrap();
    for stress in ["", "100"] {
        let mut cmd = Command::new(&host);
        cmd.current_dir(&dir);
        if !stress.is_empty() {
            cmd.env("SN_GC_STRESS", stress);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "host failed:\n{}", String::from_utf8_lossy(&out.stderr));
        let got = String::from_utf8_lossy(&out.stdout).replace("game.sn:", "game.sn:");
        assert_eq!(got, expected, "output differs (GC stress {:?})", stress);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

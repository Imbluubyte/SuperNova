//! snc — the SuperNova compiler driver.
//!
//!   snc check   <file.sn>            parse and type-check only
//!   snc emit-c  <file.sn>            print the generated C
//!   snc build   <file.sn> [-o out]   compile to a native executable
//!   snc run     <file.sn>            build to a temp dir and run
//!   snc doctor                       check that this installation works
//!   snc version

mod ast;
mod codegen;
mod lexer;
mod parser;

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const RUNTIME_H: &str = include_str!("../../runtime/snrt.h");
const GC_H: &str = include_str!("../../runtime/sngc.h");

fn usage() -> ! {
    eprintln!(
        "usage:\n  snc check  <file.sn>\n  snc emit-c <file.sn>\n  snc build  <file.sn> [-o output]\n  snc run    <file.sn>\n  snc doctor     (check this installation)\n  snc version"
    );
    exit(2);
}

fn report(path: &str, src: &str, e: &lexer::CompileError) -> ! {
    eprintln!("{}:{}:{}: error: {}", path, e.span.line, e.span.col, e.msg);
    if let Some(l) = src.lines().nth((e.span.line as usize).saturating_sub(1)) {
        eprintln!("    {}", l);
        eprintln!("    {}^", " ".repeat((e.span.col as usize).saturating_sub(1)));
    }
    exit(1);
}

fn compile_to_c(path: &str) -> String {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", path, e);
            exit(1);
        }
    };
    let module = match parser::parse(&src) {
        Ok(m) => m,
        Err(e) => report(path, &src, &e),
    };
    let file = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    match codegen::generate(&module, &file) {
        Ok(c) => c,
        Err(e) => report(path, &src, &e),
    }
}

/// Find a working C compiler: $CC, then clang, cc, gcc (Termux ships clang).
fn find_cc_opt() -> Option<String> {
    if let Ok(cc) = std::env::var("CC") {
        if !cc.is_empty() {
            let works = Command::new(&cc)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if works {
                return Some(cc);
            }
            eprintln!("warning: $CC is set to '{}' but that compiler cannot be run; trying clang, cc, gcc", cc);
        }
    }
    for cand in ["clang", "cc", "gcc"] {
        if Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(cand.to_string());
        }
    }
    None
}

fn find_cc() -> String {
    find_cc_opt().unwrap_or_else(|| {
        eprintln!("error: no C compiler found. On Termux run: pkg install clang");
        exit(1);
    })
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or("").trim().to_string()
}

const DOCTOR_BASICS: (&str, &str) = (
    include_str!("../../tests/cases/basics.sn"),
    include_str!("../../tests/cases/basics.out"),
);
const DOCTOR_COLLECTIONS: (&str, &str) = (
    include_str!("../../tests/cases/collections.sn"),
    include_str!("../../tests/cases/collections.out"),
);
const DOCTOR_GC: (&str, &str) = (
    include_str!("../../tests/cases/gc_survival.sn"),
    include_str!("../../tests/cases/gc_survival.out"),
);
const DOCTOR_CHURN: (&str, &str) = (
    include_str!("../../tests/cases/slow_gc_churn.sn"),
    include_str!("../../tests/cases/slow_gc_churn.out"),
);
const DOCTOR_OVERFLOW: &str =
    "fn deep(n: Int) -> Int:\n    var xs = [n]\n    return deep(n + 1) + xs[1]\n\nprint(\"before\")\nprint(deep(1))\n";

/// Self-test: compiles and runs real programs with this very installation and
/// reports what works, so a new platform (e.g. Termux) can be verified in one command.
fn doctor() {
    use std::time::Instant;
    let mut failures: Vec<String> = Vec::new();
    let mut report = |name: &str, ok: bool, detail: String| {
        println!("  [{}] {}{}", if ok { " ok " } else { "FAIL" }, name,
            if detail.is_empty() { String::new() } else { format!(" ({})", detail) });
        if !ok {
            failures.push(name.to_string());
        }
    };
    println!("SuperNova doctor");
    println!("  snc {}  platform {}-{}", env!("CARGO_PKG_VERSION"),
        std::env::consts::OS, std::env::consts::ARCH);

    let cc = find_cc_opt();
    match &cc {
        Some(c) => {
            let v = Command::new(c).arg("--version").output().map(|o| first_line(&o.stdout)).unwrap_or_default();
            let t = Command::new(c).arg("-dumpmachine").output().map(|o| first_line(&o.stdout)).unwrap_or_default();
            report("C compiler found", true, format!("{}; {}; {}", c, v, t));
        }
        None => {
            report("C compiler found", false, "install one, e.g. on Termux: pkg install clang".into());
            println!("\nCannot continue without a C compiler.");
            exit(1);
        }
    }

    let tmp = std::env::temp_dir().join(format!("snc-doctor-{}", std::process::id()));
    let tmp_ok = std::fs::create_dir_all(&tmp).is_ok();
    report("temporary directory writable", tmp_ok, std::env::temp_dir().display().to_string());
    if !tmp_ok {
        println!("\nSet TMPDIR to a writable directory (Termux: export TMPDIR=$PREFIX/tmp).");
        exit(1);
    }
    let exe = std::env::current_exe().expect("cannot locate snc executable");

    let run = |name: &str, src: &str, stress: bool, stats: bool| -> (bool, String, String, f64) {
        let f = tmp.join(format!("{}.sn", name));
        std::fs::write(&f, src).unwrap();
        let mut cmd = Command::new(&exe);
        cmd.arg("run").arg(&f);
        if stress {
            cmd.env("SN_GC_STRESS", "1");
        }
        if stats {
            cmd.env("SN_GC_STATS", "1");
        }
        let t = Instant::now();
        match cmd.output() {
            Ok(o) => (
                o.status.success(),
                String::from_utf8_lossy(&o.stdout).to_string(),
                String::from_utf8_lossy(&o.stderr).to_string(),
                t.elapsed().as_secs_f64(),
            ),
            Err(e) => (false, String::new(), e.to_string(), 0.0),
        }
    };

    for (name, (src, expect)) in [("basics", DOCTOR_BASICS), ("collections", DOCTOR_COLLECTIONS), ("gc_survival", DOCTOR_GC)] {
        let (ok, out, err, secs) = run(name, src, false, false);
        let good = ok && out == expect;
        let detail = if good { format!("compile + run {:.2}s", secs) } else {
            format!("{}", if !ok { err.lines().last().unwrap_or("failed").to_string() } else { "output differs from expected".to_string() })
        };
        report(&format!("program '{}' runs correctly", name), good, detail);
        if !good && ok {
            let _ = std::fs::write(tmp.join(format!("{}.actual", name)), &out);
            println!("       actual output saved to {}", tmp.join(format!("{}.actual", name)).display());
        }
        let (ok, out, err, secs) = run(name, src, true, false);
        let good = ok && out == expect;
        let detail = if good { format!("{:.2}s", secs) } else {
            if !ok { err.lines().last().unwrap_or("crashed").to_string() } else { "output differs".to_string() }
        };
        report(&format!("program '{}' survives GC on every allocation", name), good, detail);
    }

    let (ok, out, err, secs) = run("churn", DOCTOR_CHURN.0, false, true);
    let peak = err.split("peak_heap_mb=").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|v| v.parse::<f64>().ok());
    let worst = err.split("max_pause_ms=").nth(1).and_then(|r| r.split_whitespace().next()).map(|s| s.to_string());
    let good = ok && out == DOCTOR_CHURN.1 && peak.map_or(false, |p| p < 64.0);
    report("about 1 GB of garbage is reclaimed", good, format!(
        "peak heap {} MB, worst GC pause {} ms, {:.2}s",
        peak.map_or("?".to_string(), |p| format!("{:.1}", p)), worst.unwrap_or("?".into()), secs));

    let (ok, out, err, _) = run("overflow", DOCTOR_OVERFLOW, false, false);
    let good = !ok && out == "before\n" && err.contains("stack overflow");
    report("infinite recursion reports 'stack overflow'", good,
        if good { String::new() } else { err.lines().last().unwrap_or("").to_string() });

    let _ = std::fs::remove_dir_all(&tmp);
    if failures.is_empty() {
        println!("\nAll checks passed. SuperNova works on this device.");
    } else {
        println!("\n{} check(s) failed: {}", failures.len(), failures.join("; "));
        println!("Please copy this whole output when reporting the problem.");
        exit(1);
    }
}

/// A fresh scratch directory: $TMPDIR (Termux sets it), else the system temp
/// dir, else a hidden directory in the current one.
fn scratch_dir() -> PathBuf {
    let name = format!("snc-{}", std::process::id());
    let candidates = [std::env::temp_dir().join(&name), PathBuf::from(format!(".{}", name))];
    for dir in candidates {
        if std::fs::create_dir_all(&dir).is_ok() {
            return dir;
        }
    }
    eprintln!("error: cannot create a temporary directory (set TMPDIR to a writable directory)");
    exit(1);
}

fn build(path: &str, out: &Path) {
    let c = compile_to_c(path);
    let dir = scratch_dir();
    std::fs::write(dir.join("snrt.h"), RUNTIME_H).unwrap();
    std::fs::write(dir.join("sngc.h"), GC_H).unwrap();
    let c_path = dir.join("program.c");
    std::fs::write(&c_path, format!("#define _GNU_SOURCE\n#include \"snrt.h\"\n{}", c)).unwrap();
    let cc = find_cc();
    let status = Command::new(&cc)
        .args(["-O2", "-std=gnu11", "-fwrapv", "-fno-strict-aliasing", "-pthread", "-w"])
        .arg(&c_path)
        .arg("-o")
        .arg(out)
        .arg("-lm")
        .status();
    let _ = std::fs::remove_dir_all(&dir);
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => {
            eprintln!("internal error: the C compiler rejected generated code (please report this)");
            exit(1);
        }
        Err(e) => {
            eprintln!("error: failed to run {}: {}", cc, e);
            exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("version") | Some("--version") => {
            println!("snc {} ({}-{})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
            return;
        }
        Some("doctor") => {
            doctor();
            return;
        }
        _ => {}
    }
    if args.len() < 3 {
        usage();
    }
    let path = &args[2];
    match args[1].as_str() {
        "check" => {
            compile_to_c(path);
            println!("{}: OK", path);
        }
        "emit-c" => print!("{}", compile_to_c(path)),
        "build" => {
            let mut out: Option<PathBuf> = None;
            let mut i = 3;
            while i < args.len() {
                if args[i] == "-o" && i + 1 < args.len() {
                    out = Some(PathBuf::from(&args[i + 1]));
                    i += 2;
                } else {
                    usage();
                }
            }
            let out = out.unwrap_or_else(|| {
                PathBuf::from(Path::new(path).file_stem().unwrap_or_default())
            });
            build(path, &out);
        }
        "run" => {
            let exe = std::env::temp_dir().join(format!("snrun-{}", std::process::id()));
            build(path, &exe);
            let status = Command::new(&exe).status();
            let _ = std::fs::remove_file(&exe);
            match status {
                Ok(s) => exit(s.code().unwrap_or(1)),
                Err(e) => {
                    eprintln!("error: cannot run program: {}", e);
                    exit(1);
                }
            }
        }
        _ => usage(),
    }
}

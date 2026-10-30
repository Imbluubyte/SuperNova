//! snc — the SuperNova compiler driver.
//!
//!   snc check   <file.sn>            parse and type-check only
//!   snc emit-c  <file.sn>            print the generated C
//!   snc build   <file.sn> [-o out]   compile to a native executable
//!   snc run     <file.sn>            build to a temp dir and run

mod ast;
mod codegen;
mod lexer;
mod parser;

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

const RUNTIME_H: &str = include_str!("../../runtime/snrt.h");

fn usage() -> ! {
    eprintln!(
        "usage:\n  snc check  <file.sn>\n  snc emit-c <file.sn>\n  snc build  <file.sn> [-o output]\n  snc run    <file.sn>"
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
fn find_cc() -> String {
    if let Ok(cc) = std::env::var("CC") {
        if !cc.is_empty() {
            return cc;
        }
    }
    for cand in ["clang", "cc", "gcc"] {
        if Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return cand.to_string();
        }
    }
    eprintln!("error: no C compiler found. On Termux run: pkg install clang");
    exit(1);
}

fn build(path: &str, out: &Path) {
    let c = compile_to_c(path);
    let dir = std::env::temp_dir().join(format!("snc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("cannot create temp dir");
    std::fs::write(dir.join("snrt.h"), RUNTIME_H).unwrap();
    let c_path = dir.join("program.c");
    std::fs::write(&c_path, format!("#include \"snrt.h\"\n{}", c)).unwrap();
    let cc = find_cc();
    let status = Command::new(&cc)
        .args(["-O2", "-std=gnu11", "-fwrapv", "-fno-strict-aliasing", "-w"])
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

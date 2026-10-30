//! snc — the SuperNova compiler driver.

mod ast;
mod lexer;
mod parser;

use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args[1] != "check" {
        eprintln!("usage: snc check <file.sn>");
        exit(2);
    }
    let path = &args[2];
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {}: {}", path, e);
            exit(1);
        }
    };
    match parser::parse(&src) {
        Ok(m) => println!("{}: parsed OK ({} top-level items)", path, m.items.len()),
        Err(e) => {
            eprintln!("{}:{}:{}: error: {}", path, e.span.line, e.span.col, e.msg);
            exit(1);
        }
    }
}

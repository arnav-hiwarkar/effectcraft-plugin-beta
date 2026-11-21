#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: legacy-corpus [--check] [--root <dir>]";

fn run() -> Result<bool, String> {
    let mut check = false;
    let mut root = effectcraft_legacy_corpus::root();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--root" => root = PathBuf::from(args.next().ok_or(USAGE)?),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let corpus = effectcraft_legacy_corpus::generate().map_err(|e| e.to_string())?;
    if !check {
        corpus.write(&root).map_err(|e| e.to_string())?;
        println!("wrote {} members to {}", corpus.manifest().member.len(), root.display());
        return Ok(true);
    }
    for entry in &corpus.manifest().member {
        println!("{:<26} {:<24} {} file(s)  {}", entry.name, entry.kind, entry.files.len(), entry.expect);
    }
    let drift = corpus.check(&root).map_err(|e| e.to_string())?;
    for d in &drift {
        eprintln!("{d}");
    }
    if drift.is_empty() {
        println!("{} matches the generator", root.display());
    } else {
        eprintln!("{} drift(s): run `cargo xtask legacy-corpus` and review the diff", drift.len());
    }
    Ok(drift.is_empty())
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("legacy-corpus: {e}");
            ExitCode::from(2)
        }
    }
}

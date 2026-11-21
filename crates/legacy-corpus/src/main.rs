#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

fn main() -> std::process::ExitCode {
    craft_testkit::corpus::run_cli(std::env::args().skip(1), &effectcraft_legacy_corpus::root(), effectcraft_legacy_corpus::generate)
}

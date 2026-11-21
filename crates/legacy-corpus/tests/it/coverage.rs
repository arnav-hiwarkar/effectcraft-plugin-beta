use effectcraft_legacy_corpus::LegacyPath;

use crate::corpus;

#[test]
fn ret_corpus_coverage_every_legacy_path_has_a_member() {
    let kinds: Vec<&str> = LegacyPath::ALL.iter().map(|kind| kind.name()).collect();
    assert_eq!(corpus().manifest().coverage(&kinds).unwrap(), vec![]);
}

#[test]
fn the_committed_corpus_is_what_the_generator_makes() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_legacy-corpus")).arg("--check").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("matches the generator"), "{stdout}");
}

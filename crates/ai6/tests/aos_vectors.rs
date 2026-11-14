//! The specification's conformance vectors (`tests/fixtures/aos-vectors/`, copied byte for byte),
//! run by coqui-aos's runner. Each test runs every case of one file and names each failing case.

use std::path::PathBuf;

use coqui_aos::vectors::Runner;

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aos-vectors")
}

fn run(file: &str) {
    let report = Runner::new(vectors()).run_file(file).expect("a known file");
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
    assert!(report.passed > 0, "{file} ran no case");
    report.assert_clean();
}

#[test]
fn every_vector_file_has_a_test() {
    assert_eq!(Runner::new(vectors()).unrun(), Vec::<String>::new());
}

#[test]
fn catalog() {
    run("catalog.json");
}

#[test]
fn compose() {
    run("compose.json");
}

#[test]
fn digest() {
    run("digest.json");
}

#[test]
fn effective() {
    run("effective.json");
}

#[test]
fn front_matter() {
    run("front_matter.json");
}

#[test]
fn hooks() {
    run("hooks.json");
}

#[test]
fn names() {
    run("names.json");
}

#[test]
fn reading() {
    run("reading.json");
}

#[test]
fn skills() {
    run("skills.json");
}

#[test]
fn source() {
    run("source.json");
}

#[test]
fn templating() {
    run("templating.json");
}

#[test]
fn validation() {
    run("validation.json");
}

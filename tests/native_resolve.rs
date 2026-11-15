use std::fs;
use std::path::PathBuf;

use oncebitten::engine::native::resolve_in;
use tempfile::TempDir;

fn pathext() -> Vec<String> {
    [".COM", ".EXE", ".CMD"].map(String::from).to_vec()
}

#[test]
fn cmd_shim_resolves_through_pathext() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("npx.CMD"), "@echo off\n").unwrap();

    let found = resolve_in("npx", &[dir.path().to_path_buf()], &pathext());

    assert_eq!(found, Some(dir.path().join("npx.CMD")));
}

#[test]
fn extensionless_script_is_skipped_when_pathext_applies() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("npx"), "#!/bin/sh\n").unwrap();
    fs::write(dir.path().join("npx.CMD"), "@echo off\n").unwrap();

    let found = resolve_in("npx", &[dir.path().to_path_buf()], &pathext());

    assert_eq!(found, Some(dir.path().join("npx.CMD")));
}

#[test]
fn first_directory_on_path_wins() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    fs::write(first.path().join("eslint.EXE"), "").unwrap();
    fs::write(second.path().join("eslint.CMD"), "").unwrap();
    let dirs: Vec<PathBuf> = vec![first.path().into(), second.path().into()];

    let found = resolve_in("eslint", &dirs, &pathext());

    assert_eq!(found, Some(first.path().join("eslint.EXE")));
}

#[test]
fn missing_program_resolves_to_none() {
    let dir = TempDir::new().unwrap();

    let found = resolve_in("eslint", &[dir.path().to_path_buf()], &pathext());

    assert_eq!(found, None);
}

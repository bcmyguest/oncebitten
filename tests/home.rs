use std::collections::BTreeMap;
use std::path::PathBuf;

use oncebitten::config::{home_dir, Platform};

fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn unix_uses_home_only() {
    let map = vars(&[("HOME", "/home/b"), ("USERPROFILE", r"C:\Users\b")]);
    assert_eq!(
        home_dir(&map, Platform::Unix),
        Some(PathBuf::from("/home/b"))
    );
    assert_eq!(
        home_dir(&vars(&[("USERPROFILE", r"C:\Users\b")]), Platform::Unix),
        None
    );
}

#[test]
fn windows_prefers_home_when_set() {
    let map = vars(&[("HOME", r"C:\msys\home\b"), ("USERPROFILE", r"C:\Users\b")]);
    assert_eq!(
        home_dir(&map, Platform::Windows),
        Some(PathBuf::from(r"C:\msys\home\b"))
    );
}

#[test]
fn windows_falls_back_to_userprofile() {
    let map = vars(&[("USERPROFILE", r"C:\Users\b")]);
    assert_eq!(
        home_dir(&map, Platform::Windows),
        Some(PathBuf::from(r"C:\Users\b"))
    );
}

#[test]
fn windows_falls_back_to_homedrive_and_homepath() {
    let map = vars(&[("HOMEDRIVE", "C:"), ("HOMEPATH", r"\Users\b")]);
    assert_eq!(
        home_dir(&map, Platform::Windows),
        Some(PathBuf::from(r"C:\Users\b"))
    );
}

#[test]
fn windows_skips_relative_values() {
    let map = vars(&[
        ("HOME", "home/b"),
        ("USERPROFILE", ""),
        ("HOMEDRIVE", "C:"),
        ("HOMEPATH", r"\Users\b"),
    ]);
    assert_eq!(
        home_dir(&map, Platform::Windows),
        Some(PathBuf::from(r"C:\Users\b"))
    );
}

#[test]
fn windows_without_any_home_variable_has_none() {
    assert_eq!(
        home_dir(&vars(&[("HOMEDRIVE", "C:")]), Platform::Windows),
        None
    );
    assert_eq!(home_dir(&vars(&[]), Platform::Windows), None);
}

#[test]
fn windows_config_falls_back_to_home_appdata_roaming() {
    let map = vars(&[("USERPROFILE", r"C:\Users\b")]);
    let path = oncebitten::config::user_config_path_for(&map, Platform::Windows).unwrap();
    assert!(
        path.starts_with(PathBuf::from(r"C:\Users\b").join("AppData").join("Roaming")),
        "{path:?}"
    );
}

#[test]
fn windows_config_prefers_absolute_appdata() {
    let map = vars(&[("USERPROFILE", r"C:\Users\b"), ("APPDATA", r"D:\Roam")]);
    let path = oncebitten::config::user_config_path_for(&map, Platform::Windows).unwrap();
    assert!(path.starts_with(r"D:\Roam"), "{path:?}");
}

#[test]
fn windows_data_dir_falls_back_to_home_appdata_local() {
    let map = vars(&[("HOME", r"C:\Users\b")]);
    let path = oncebitten::store::events_db_path_for(&map, Platform::Windows).unwrap();
    assert!(
        path.starts_with(PathBuf::from(r"C:\Users\b").join("AppData").join("Local")),
        "{path:?}"
    );
}

#[test]
fn windows_without_any_base_dir_is_an_error() {
    let map = vars(&[("APPDATA", "relative")]);
    assert!(oncebitten::config::user_config_path_for(&map, Platform::Windows).is_err());
    assert!(oncebitten::store::events_db_path_for(&map, Platform::Windows).is_err());
}

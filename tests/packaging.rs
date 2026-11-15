use std::fs;

use serde_json::Value;

const HOSTS: [(&str, &str, &str); 4] = [
    ("claude", "hooks/hooks.json", "CLAUDE_PLUGIN_ROOT"),
    ("codex", "hooks/hooks.json", "PLUGIN_ROOT"),
    ("cursor", "hooks/hooks.json", "CURSOR_PLUGIN_ROOT"),
    ("copilot", "hooks/hooks.json", "PLUGIN_ROOT"),
];

fn commands(value: &Value) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![value];
    while let Some(node) = stack.pop() {
        match node {
            Value::Object(map) => {
                if let Some(command) = map.get("command").and_then(Value::as_str) {
                    found.push(command.to_string());
                }
                stack.extend(map.values());
            }
            Value::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    found
}

#[test]
fn plugin_hooks_use_the_plugin_root_variable_and_the_right_host() {
    for (host, file, var) in HOSTS {
        let path = format!("{}/packaging/{host}/{file}", env!("CARGO_MANIFEST_DIR"));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let found = commands(&value);
        assert!(!found.is_empty(), "{host}");
        let fetch = format!("\"${{{var}}}/bin/oncebitten\" --version > /dev/null");
        for command in found.into_iter().filter(|c| *c != fetch) {
            assert_eq!(
                command,
                format!("\"${{{var}}}/bin/oncebitten\" capture --host {host}")
            );
            assert!(!command.contains(":-"), "{host}: POSIX-only default syntax");
        }
    }
}

#[test]
fn plugin_manifests_parse_and_point_at_the_hooks_file() {
    for (host, manifest) in [
        ("claude", ".claude-plugin/plugin.json"),
        ("codex", ".codex-plugin/plugin.json"),
        ("cursor", ".cursor-plugin/plugin.json"),
        ("copilot", ".github/plugin/plugin.json"),
    ] {
        let path = format!("{}/packaging/{host}/{manifest}", env!("CARGO_MANIFEST_DIR"));
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["name"], "oncebitten", "{host}");
        assert!(value["hooks"]
            .as_str()
            .unwrap()
            .ends_with("hooks/hooks.json"));
    }
}

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn json(path: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root().join(path)).unwrap()).unwrap()
}

#[test]
fn marketplace_lists_the_claude_plugin_under_its_manifest_name() {
    let marketplace = json(".claude-plugin/marketplace.json");
    assert!(marketplace["owner"]["name"].is_string());
    let entry = &marketplace["plugins"][0];
    let source = entry["source"].as_str().unwrap();
    assert!(source.starts_with("./") && !source.contains(".."));
    let manifest = json(&format!("{source}/.claude-plugin/plugin.json"));
    assert_eq!(entry["name"], manifest["name"]);
}

#[test]
fn plugin_versions_and_the_launcher_match_the_crate() {
    let version = env!("CARGO_PKG_VERSION");
    for manifest in [
        "packaging/claude/.claude-plugin/plugin.json",
        "packaging/codex/.codex-plugin/plugin.json",
        "packaging/cursor/.cursor-plugin/plugin.json",
        "packaging/copilot/.github/plugin/plugin.json",
    ] {
        assert_eq!(json(manifest)["version"], version, "{manifest}");
    }
    let launcher = fs::read_to_string(root().join("packaging/claude/bin/oncebitten")).unwrap();
    assert!(launcher.contains(&format!("\nversion={version}\n")));
}

#[test]
fn the_plugin_installer_is_a_copy_of_install_sh() {
    assert_eq!(
        fs::read(root().join("install.sh")).unwrap(),
        fs::read(root().join("packaging/claude/scripts/install.sh")).unwrap()
    );
}

#[cfg(unix)]
mod release {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output, Stdio};

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    const FAKE: &str = "#!/bin/sh\necho \"fake $*\"\n";

    fn target() -> String {
        let out = |arg| {
            let o = Command::new("uname").arg(arg).output().unwrap();
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        let arch = match out("-m").as_str() {
            "arm64" | "aarch64" => "aarch64",
            _ => "x86_64",
        };
        match out("-s").as_str() {
            "Darwin" => format!("{arch}-apple-darwin"),
            _ => format!("{arch}-unknown-linux-musl"),
        }
    }

    /// A releases tree as GitHub serves it: download/v<version>/<asset>.
    fn publish(dir: &Path, version: &str, corrupt: bool) -> String {
        let stage = dir.join("stage");
        fs::create_dir_all(&stage).unwrap();
        for name in ["oncebitten", "oncebitten-checker"] {
            fs::write(stage.join(name), FAKE).unwrap();
        }
        let assets = dir.join("releases/download").join(format!("v{version}"));
        fs::create_dir_all(&assets).unwrap();
        let name = format!("oncebitten-{}.tar.gz", target());
        let archive = assets.join(&name);
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .args(["oncebitten", "oncebitten-checker"])
            .status()
            .unwrap();
        assert!(status.success());
        let mut digest: String = Sha256::digest(fs::read(&archive).unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if corrupt {
            digest = "0".repeat(64);
        }
        fs::write(
            assets.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .unwrap();
        format!("file://{}", dir.join("releases").display())
    }

    fn run(script: &Path, args: &[&str], env: &[(&str, &Path)], url: &str, stdin: &str) -> Output {
        let mut cmd = Command::new("sh");
        cmd.arg(script)
            .args(args)
            .env_remove("CLAUDE_PLUGIN_DATA")
            .env("ONCEBITTEN_RELEASE_URL", url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn script(path: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
    }

    #[test]
    fn install_sh_installs_a_verified_release() {
        let dir = TempDir::new().unwrap();
        let url = publish(dir.path(), "9.9.9", false);
        let bin = dir.path().join("bin");
        let out = Command::new("sh")
            .arg(script("install.sh"))
            .env("ONCEBITTEN_RELEASE_URL", &url)
            .env("ONCEBITTEN_VERSION", "9.9.9")
            .env("ONCEBITTEN_INSTALL_DIR", &bin)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        for name in ["oncebitten", "oncebitten-checker"] {
            let mode = fs::metadata(bin.join(name)).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{name} is not executable");
        }
        assert_eq!(
            fs::read_dir(&bin).unwrap().count(),
            2,
            "temp dir left behind"
        );
    }

    #[test]
    fn install_sh_refuses_a_checksum_mismatch() {
        let dir = TempDir::new().unwrap();
        let url = publish(dir.path(), "9.9.9", true);
        let bin = dir.path().join("bin");
        let out = Command::new("sh")
            .arg(script("install.sh"))
            .env("ONCEBITTEN_RELEASE_URL", &url)
            .env("ONCEBITTEN_VERSION", "9.9.9")
            .env("ONCEBITTEN_INSTALL_DIR", &bin)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"));
        assert!(!bin.join("oncebitten").exists());
    }

    #[test]
    fn launcher_fetches_its_version_then_runs_it() {
        let dir = TempDir::new().unwrap();
        let version = env!("CARGO_PKG_VERSION");
        let url = publish(dir.path(), version, false);
        let data = dir.path().join("data");
        let launcher = script("packaging/claude/bin/oncebitten");
        let env = [("CLAUDE_PLUGIN_DATA", data.as_path())];
        let out = run(&launcher, &["evidence", "list"], &env, &url, "");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "fake evidence list\n");
        assert!(data.join("bin").join(version).join("oncebitten").is_file());
    }

    #[test]
    fn launcher_capture_never_waits_for_a_download() {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let launcher = script("packaging/claude/bin/oncebitten");
        let env = [("CLAUDE_PLUGIN_DATA", data.as_path())];
        let out = run(
            &launcher,
            &["capture", "--host", "claude"],
            &env,
            "file:///missing",
            "{}",
        );
        assert!(out.status.success());
        assert!(out.stdout.is_empty());
    }
}

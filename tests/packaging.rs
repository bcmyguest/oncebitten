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
        for command in found {
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

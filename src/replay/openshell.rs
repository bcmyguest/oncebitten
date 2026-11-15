//! Runs the replay inside an OpenShell sandbox.
//!
//! The sandbox cannot mount host files, so the checker, the rule and the
//! corpus are baked into a per-replay image. The image is read-only inside the
//! sandbox through the policy. Every sandbox created is deleted, even when
//! create or exec fails.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Backend, RawRun, ReplaySpec};

pub const DEFAULT_BASE_IMAGE: &str = "nvcr.io/nvidia/base/ubuntu:24.04";
pub const CHECKER_IN_SANDBOX: &str = "/opt/oncebitten/checker";
pub const RULE_IN_SANDBOX: &str = "/rule/check.yml";
pub const CORPUS_IN_SANDBOX: &str = "/corpus";

const POLICY: &str = "version: 1
filesystem_policy:
  include_workdir: false
  read_only:
    - /corpus
    - /rule
    - /opt
    - /bin
    - /usr
    - /lib
    - /etc
    - /proc
    - /dev/urandom
  read_write:
    - /sandbox
    - /tmp
    - /dev/null
network_policies: {}
";

/// The OpenShell CLI and the image it starts sandboxes from.
pub struct OpenShell {
    pub binary: String,
    pub gateway: Option<String>,
    pub base_image: String,
}

impl OpenShell {
    pub fn new(gateway: Option<String>) -> Self {
        Self {
            binary: "openshell".to_owned(),
            gateway,
            base_image: DEFAULT_BASE_IMAGE.to_owned(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        if let Some(endpoint) = &self.gateway {
            command.args(["--gateway-endpoint", endpoint]);
        }
        command.args(args);
        command
    }
}

impl Backend for OpenShell {
    fn run(&self, spec: &ReplaySpec, files: &[PathBuf]) -> Result<Vec<RawRun>, String> {
        let work = tempfile::tempdir().map_err(|error| format!("temp dir: {error}"))?;
        let context = build_context(work.path(), &self.base_image, spec, files)?;
        let stamp = unique_stamp();
        let image = format!("oncebitten-replay:{stamp}");
        let sandbox = sandbox_name();

        let result = build_image(&image, &context).and_then(|()| {
            self.create_sandbox(&sandbox, &image, &work.path().join("policy.yaml"))?;
            (0..files.len())
                .map(|index| self.exec_checker(&sandbox, index, &files[index]))
                .collect::<Result<Vec<RawRun>, String>>()
        });

        // Cleanup runs on success and failure alike. Failures here do not change the verdict.
        let _ = self.command(&["sandbox", "delete", &sandbox]).output();
        let _ = Command::new("docker")
            .args(["rmi", "--force", &image])
            .output();
        result
    }
}

impl OpenShell {
    fn create_sandbox(&self, sandbox: &str, image: &str, policy: &Path) -> Result<(), String> {
        std::fs::write(policy, POLICY).map_err(|error| format!("writing policy: {error}"))?;
        let policy = policy.display().to_string();
        let output = self
            .command(&[
                "sandbox",
                "create",
                "--name",
                sandbox,
                "--from",
                image,
                "--detach",
                "--no-auto-providers",
                "--no-tty",
                "--policy",
                &policy,
            ])
            .output()
            .map_err(|error| format!("sandbox create: starting {}: {error}", self.binary))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "sandbox create failed (exit {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn exec_checker(&self, sandbox: &str, index: usize, file: &Path) -> Result<RawRun, String> {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let target = format!("{CORPUS_IN_SANDBOX}/{index}-{name}");
        let output = self
            .command(&[
                "sandbox",
                "exec",
                "--no-tty",
                sandbox,
                "--",
                CHECKER_IN_SANDBOX,
                RULE_IN_SANDBOX,
                &target,
            ])
            .output()
            .map_err(|error| format!("sandbox exec: starting {}: {error}", self.binary))?;
        Ok(RawRun {
            target,
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Writes the Dockerfile, checker, rule and corpus into `root/context`.
/// Returns the build context directory.
fn build_context(
    root: &Path,
    base_image: &str,
    spec: &ReplaySpec,
    files: &[PathBuf],
) -> Result<PathBuf, String> {
    let context = root.join("context");
    let io = |error: std::io::Error| format!("building the image context: {error}");
    std::fs::create_dir_all(context.join("rule")).map_err(io)?;
    std::fs::create_dir_all(context.join("corpus")).map_err(io)?;
    std::fs::copy(&spec.checker, context.join("checker")).map_err(io)?;
    std::fs::copy(&spec.rule, context.join("rule/check.yml")).map_err(io)?;
    for (index, file) in files.iter().enumerate() {
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        std::fs::copy(file, context.join(format!("corpus/{index}-{name}"))).map_err(io)?;
    }
    let dockerfile = format!(
        "FROM {base_image}\nCOPY checker {CHECKER_IN_SANDBOX}\nCOPY rule /rule\nCOPY corpus {CORPUS_IN_SANDBOX}\n"
    );
    std::fs::write(context.join("Dockerfile"), dockerfile).map_err(io)?;
    Ok(context)
}

fn build_image(image: &str, context: &Path) -> Result<(), String> {
    let output = Command::new("docker")
        .arg("build")
        .args(["-t", image])
        .arg(context)
        .output()
        .map_err(|error| format!("image build: starting docker: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "image build failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The gateway rejects names longer than 19 characters.
fn sandbox_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("obr-{}-{count}", std::process::id())
}

fn unique_stamp() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

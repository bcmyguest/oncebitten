use std::fmt;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::engine::native::spawn_path;
use crate::triage::host::{verdict_from, Captured, Invocation};

/// Set on every triage call; each oncebitten hook entry point exits 0 on it.
pub const TRIAGE_ENV: &str = "ONCEBITTEN_TRIAGE";
const POLL: Duration = Duration::from_millis(20);

/// The call never produced output to judge: nothing ran, it hung, or it was
/// killed. The caller reports these as "could not run".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Missing(String),
    Spawn(String),
    Timeout,
    Io(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Missing(program) => write!(f, "host program `{program}` was not found"),
            RunError::Spawn(message) => write!(f, "cannot start host: {message}"),
            RunError::Timeout => write!(f, "host call timed out"),
            RunError::Io(message) => write!(f, "host call failed: {message}"),
        }
    }
}

impl std::error::Error for RunError {}

/// Runs one headless call. Tests substitute a fake; `ProcessRunner` spawns.
pub trait Runner {
    fn run(
        &self,
        inv: &Invocation,
        workdir: &Path,
        timeout: Duration,
    ) -> Result<Captured, RunError>;
}

/// Spawns the host with stdin closed, the triage switch set, and the given
/// workdir as its current directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(
        &self,
        inv: &Invocation,
        workdir: &Path,
        timeout: Duration,
    ) -> Result<Captured, RunError> {
        for (path, contents) in &inv.files {
            fs::write(path, contents).map_err(|e| RunError::Io(e.to_string()))?;
        }
        let mut child = spawn(inv, workdir)?;
        let stdout = Arc::new(Mutex::new(Vec::new()));
        if let Some(pipe) = child.stdout.take() {
            drain(pipe, Arc::clone(&stdout));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if child
                .try_wait()
                .map_err(|e| RunError::Io(e.to_string()))?
                .is_some()
            {
                break;
            }
            if Instant::now() >= deadline {
                kill_tree(&mut child);
                return Err(RunError::Timeout);
            }
            if inv.streaming && answered(inv, &snapshot(&stdout)) {
                kill_tree(&mut child);
                break;
            }
            thread::sleep(POLL);
        }
        Ok(Captured {
            stdout: snapshot(&stdout),
            answer: inv
                .answer_file
                .as_deref()
                .and_then(|path| fs::read_to_string(path).ok()),
        })
    }
}

/// Kills the host and its descendants. On Windows an npm `.cmd` wrapper runs
/// the real host as a child, and killing only the wrapper orphans it.
fn kill_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn(inv: &Invocation, workdir: &Path) -> Result<Child, RunError> {
    let program = spawn_path(&inv.program);
    let mut command = Command::new(program);
    command
        .args(&inv.args)
        .current_dir(workdir)
        .env(TRIAGE_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.spawn().map_err(|err| match err.kind() {
        ErrorKind::NotFound => RunError::Missing(inv.program.clone()),
        _ => RunError::Spawn(err.to_string()),
    })
}

/// Reads the pipe on its own thread so a full pipe cannot stall the host.
fn drain(mut pipe: impl Read + Send + 'static, into: Arc<Mutex<Vec<u8>>>) {
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(read) = pipe.read(&mut chunk) {
            if read == 0 {
                break;
            }
            if let Ok(mut buf) = into.lock() {
                buf.extend_from_slice(&chunk[..read]);
            }
        }
    });
}

fn snapshot(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    buf.lock()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn answered(inv: &Invocation, stdout: &str) -> bool {
    verdict_from(
        inv,
        &Captured {
            stdout: stdout.to_string(),
            answer: None,
        },
    )
    .is_ok()
}

//! Test double for `gh`, `glab` and `az`. A test copies this binary into a temp
//! directory under the CLI's name. Each run appends its argv as a JSON array
//! to `calls.log` beside the copy, prints the file `reply`, and exits with the
//! number in `status` (default 0), printing `stderr` when it is set. For call
//! number N (from 0), `reply.N`, `status.N` and `stderr.N` win over the plain
//! names.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

fn read_optional(dir: &Path, name: &str, call: usize) -> String {
    fs::read_to_string(dir.join(format!("{name}.{call}")))
        .or_else(|_| fs::read_to_string(dir.join(name)))
        .unwrap_or_default()
}

fn main() {
    let exe = std::env::current_exe().expect("fake forge knows its own path");
    let dir = exe.parent().expect("fake forge lives in a directory");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let line = serde_json::to_string(&args).expect("argv serialises");
    let call = fs::read_to_string(dir.join("calls.log"))
        .map(|log| log.lines().count())
        .unwrap_or(0);
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("calls.log"))
        .expect("calls.log opens");
    writeln!(log, "{line}").expect("calls.log writes");

    print!("{}", read_optional(dir, "reply", call));
    let stderr = read_optional(dir, "stderr", call);
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    let status: i32 = read_optional(dir, "status", call)
        .trim()
        .parse()
        .unwrap_or(0);
    std::process::exit(status);
}

use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::host::{self, Host};
use crate::signal;
use crate::store::{self, Store};

const HISTORY_LIMIT: usize = 500;
const TRIAGE_ENV: &str = "ONCEBITTEN_TRIAGE";

pub fn run(host: &str, payload: &str, db: &Path, ts: i64) -> Result<Option<i64>> {
    let host = Host::parse(host)?;
    let Some(event) = host::normalize(host, payload, ts)? else {
        return Ok(None);
    };
    let store = Store::open(db)?;
    let history = store.session_history(&event.host, &event.session_id, HISTORY_LIMIT)?;
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals)
}

pub fn run_from_process(host: &str) -> Result<()> {
    if std::env::var(TRIAGE_ENV).as_deref() == Ok("1") {
        return Ok(());
    }
    let mut payload = String::new();
    std::io::stdin()
        .read_to_string(&mut payload)
        .context("cannot read hook payload from stdin")?;
    let db = store::events_db_path()?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis() as i64;
    run(host, &payload, &db, ts)?;
    Ok(())
}

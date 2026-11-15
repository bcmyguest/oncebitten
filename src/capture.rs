use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config;
use crate::host::{self, Host};
use crate::signal;
use crate::store::{self, Store};

const HISTORY_LIMIT: usize = 500;
const TRIAGE_ENV: &str = "ONCEBITTEN_TRIAGE";

pub fn run(host: &str, payload: &str, db: &Path, ts: i64) -> Result<Option<i64>> {
    run_with(host, payload, db, ts, signal::DEFAULT_CORRECTION_LEXICON)
}

pub fn run_with<P: AsRef<str>>(
    host: &str,
    payload: &str,
    db: &Path,
    ts: i64,
    lexicon: &[P],
) -> Result<Option<i64>> {
    let host = Host::parse(host)?;
    let Some(event) = host::normalize(host, payload, ts)? else {
        return Ok(None);
    };
    let store = Store::open(db)?;
    let history = store.session_history(&event.host, &event.session_id, HISTORY_LIMIT)?;
    let signals = signal::derive_with(&event, &history, lexicon);
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
    let loaded = config::load(&config::Inputs::from_process()?)?;
    let lexicon: Vec<String> = loaded
        .config
        .capture
        .correction_phrases
        .into_iter()
        .chain(loaded.config.capture.extra_correction_phrases)
        .collect();
    run_with(host, &payload, &db, ts, &lexicon)?;
    Ok(())
}

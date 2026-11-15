use oncebitten::store::{HitsRow, Store};
use tempfile::TempDir;

fn sample_hits() -> HitsRow {
    HitsRow {
        count: 2,
        files: 1,
        samples: vec!["src/lib.rs:3: x.unwrap()".to_owned()],
    }
}

#[test]
fn an_old_proposals_table_gains_the_hit_columns() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    {
        let store = Store::open(&path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE proposals;
             CREATE TABLE proposals (cluster TEXT PRIMARY KEY, ts INTEGER NOT NULL, repo TEXT NOT NULL, kind TEXT NOT NULL, state TEXT NOT NULL, rule TEXT, dir TEXT, proven INTEGER NOT NULL, note TEXT, verdict TEXT NOT NULL, rationale TEXT, occurrences TEXT NOT NULL);
             INSERT INTO proposals VALUES ('c1', 1, '/r', 'finding', 'waiting', NULL, NULL, 0, NULL, '{}', NULL, '[]');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let row = store.proposal("c1").unwrap().unwrap();
    assert!(row.hits.is_none());
    store.set_proposal_hits("c1", &sample_hits()).unwrap();
    drop(store);
    let reopened = Store::open(&path).unwrap();
    let row = reopened.proposal("c1").unwrap().unwrap();
    assert_eq!(row.hits, Some(sample_hits()));
}

#[test]
fn set_proposal_hits_names_an_unknown_proposal() {
    let store = Store::open_in_memory().unwrap();
    let error = store.set_proposal_hits("nope", &sample_hits()).unwrap_err();
    assert!(error.to_string().contains("no proposal `nope`"), "{error}");
}

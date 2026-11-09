// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: Apache-2.0

//! Journal tests that need the crate-private flip hook.

use std::fs;
use std::path::Path;

use crate::journal::{read_journal, JournalOp};
use crate::*;

fn setup() -> (tempfile::TempDir, Home, InstallerLock) {
    let t = tempfile::tempdir().unwrap();
    let h = Home::at(t.path().join(".cigscript"));
    h.ensure_layout().unwrap();
    let l = h.lock().unwrap();
    (t, h, l)
}

fn idx(machine: &str) -> String {
    let i = ShimIndex {
        machine: Some(machine.to_string()),
        ..ShimIndex::default()
    };
    i.render()
}

fn live_index(h: &Home) -> Option<String> {
    fs::read_to_string(h.shim_index_path()).ok()
}

/// A committed baseline so a later flip has a "previous".
fn baseline(h: &Home, l: &InstallerLock, machine: &str) {
    let mut t = Txn::begin(h, "baseline", l).unwrap();
    t.flip(FlipIntent {
        index: idx(machine),
    })
    .unwrap();
    t.write(
        &h.state_path(),
        format!("{{\"m\":\"{machine}\"}}").as_bytes(),
        None,
    )
    .unwrap();
    t.commit().unwrap();
}

fn lines(h: &Home, id: &str) -> Vec<JournalOp> {
    read_journal(id, &h.txn_root().join(id).join("journal.jsonl"))
        .unwrap()
        .lines
        .into_iter()
        .map(|l| l.op)
        .collect()
}

#[test]
fn journal_lines_have_the_documented_shape() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "install carton 0.1.0", &l).unwrap();
    let id = t.id().to_string();
    assert_eq!(id.len(), "20261112T100311-a1b2c3".len());
    t.write(&h.state_path(), b"{}", None).unwrap();
    t.flip(FlipIntent {
        index: idx("0.1.0"),
    })
    .unwrap();
    t.commit().unwrap();
    let text = fs::read_to_string(h.txn_root().join(&id).join("journal.jsonl")).unwrap();
    let mut it = text.lines();
    let begin = it.next().unwrap();
    assert!(
        begin.starts_with(r#"{"seq":0,"t":"begin","label":"install carton 0.1.0","pid":"#),
        "{begin}"
    );
    assert!(begin.ends_with(r#""format":1}"#));
    let write = it.next().unwrap();
    assert!(write.starts_with(r#"{"seq":1,"t":"write","path":""#));
    assert!(write.contains(r#""before":{"state":"absent"}"#));
    assert!(write.contains(r#""after_sha256":""#));
    assert!(it
        .next()
        .unwrap()
        .starts_with(r#"{"seq":2,"t":"flip","index_sha256":""#));
    assert_eq!(it.next().unwrap(), r#"{"seq":3,"t":"commit"}"#);
    assert_eq!(it.next(), None);
}

#[test]
fn recover_flip_logged_rename_not_done_rolls_back() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let before_index = live_index(&h).unwrap();
    let before_state = fs::read(h.state_path()).unwrap();

    let mut t = Txn::begin(&h, "update", &l).unwrap();
    let id = t.id().to_string();
    t.stop_before_rename();
    t.write(&h.state_path(), b"{\"m\":\"0.2.0\"}", None)
        .unwrap();
    t.flip(FlipIntent {
        index: idx("0.2.0"),
    })
    .unwrap();
    drop(t); // the process "dies" here
    assert!(h.installer().join("shim-index.new").exists());
    assert_eq!(live_index(&h).unwrap(), before_index);

    let r = recover(&h, &l).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].txn, id);
    assert_eq!(r[0].code(), "E1832");
    assert!(matches!(r[0].outcome, RecoverOutcome::RolledBack { .. }));
    assert_eq!(live_index(&h).unwrap(), before_index);
    assert_eq!(fs::read(h.state_path()).unwrap(), before_state);
    assert!(!h.installer().join("shim-index.new").exists());
    assert!(lines(&h, &id).last() == Some(&JournalOp::RollbackDone));
    assert!(
        recover(&h, &l).unwrap().is_empty(),
        "second recover has nothing to do"
    );
}

#[test]
fn recover_flip_rename_done_rolls_forward() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let mut t = Txn::begin(&h, "update", &l).unwrap();
    let id = t.id().to_string();
    t.write(&h.state_path(), b"{\"m\":\"0.2.0\"}", None)
        .unwrap();
    t.flip(FlipIntent {
        index: idx("0.2.0"),
    })
    .unwrap();
    drop(t); // died before the commit line

    let r = recover(&h, &l).unwrap();
    assert_eq!(r[0].outcome, RecoverOutcome::RolledForward);
    assert_eq!(live_index(&h).unwrap(), idx("0.2.0"));
    assert_eq!(fs::read(h.state_path()).unwrap(), b"{\"m\":\"0.2.0\"}");
    assert_eq!(
        lines(&h, &id).last(),
        Some(&JournalOp::Commit { recovered: true })
    );
    assert!(recover(&h, &l).unwrap().is_empty());
}

#[test]
fn recover_no_flip_rolls_back_everything() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let state = fs::read(h.state_path()).unwrap();
    let mut t = Txn::begin(&h, "install", &l).unwrap();
    t.write(&h.state_path(), b"changed", None).unwrap();
    t.write(&h.installer().join("new-file"), b"x", None)
        .unwrap();
    drop(t);
    let r = recover(&h, &l).unwrap();
    assert!(matches!(
        r[0].outcome,
        RecoverOutcome::RolledBack { undone: 2 }
    ));
    assert_eq!(fs::read(h.state_path()).unwrap(), state);
    assert!(!h.installer().join("new-file").exists());
}

#[test]
fn recover_unrelated_index_is_corrupt_and_touches_nothing() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let mut t = Txn::begin(&h, "update", &l).unwrap();
    t.stop_before_rename();
    t.write(&h.state_path(), b"changed", None).unwrap();
    t.flip(FlipIntent {
        index: idx("0.2.0"),
    })
    .unwrap();
    drop(t);
    fs::write(h.shim_index_path(), "somebody else wrote this\n").unwrap();
    let r = recover(&h, &l).unwrap();
    assert!(matches!(r[0].outcome, RecoverOutcome::Corrupt { .. }));
    assert_eq!(r[0].code(), "E1891");
    assert!(r[0].is_problem());
    assert_eq!(
        fs::read(h.state_path()).unwrap(),
        b"changed",
        "nothing was touched"
    );
    // and it stays reported
    assert!(matches!(
        recover(&h, &l).unwrap()[0].outcome,
        RecoverOutcome::Corrupt { .. }
    ));
}

#[test]
fn identical_index_flip_is_rolled_back_not_forward() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let state = fs::read(h.state_path()).unwrap();
    let mut t = Txn::begin(&h, "repair", &l).unwrap();
    t.write(&h.state_path(), b"changed", None).unwrap();
    t.flip(FlipIntent {
        index: idx("0.1.0"),
    })
    .unwrap();
    drop(t);
    let r = recover(&h, &l).unwrap();
    assert!(matches!(r[0].outcome, RecoverOutcome::RolledBack { .. }));
    assert_eq!(fs::read(h.state_path()).unwrap(), state);
}

#[test]
fn torn_last_line_is_dropped_and_recovery_goes_on() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "install", &l).unwrap();
    let id = t.id().to_string();
    t.write(&h.state_path(), b"one", None).unwrap();
    drop(t);
    let jp = h.txn_root().join(&id).join("journal.jsonl");
    let mut text = fs::read_to_string(&jp).unwrap();
    text.push_str(r#"{"seq":2,"t":"wri"#);
    fs::write(&jp, text).unwrap();
    let r = recover(&h, &l).unwrap();
    assert!(matches!(
        r[0].outcome,
        RecoverOutcome::RolledBack { undone: 1 }
    ));
    assert!(!h.state_path().exists());
    // the journal is readable again and ends in rollback-done
    assert_eq!(lines(&h, &id).last(), Some(&JournalOp::RollbackDone));
}

#[test]
fn garbage_in_the_middle_is_corrupt() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "install", &l).unwrap();
    let id = t.id().to_string();
    t.write(&h.state_path(), b"one", None).unwrap();
    t.write(&h.config_path(), b"two", None).unwrap();
    drop(t);
    let jp = h.txn_root().join(&id).join("journal.jsonl");
    let text = fs::read_to_string(&jp)
        .unwrap()
        .replacen(r#""t":"write""#, r#""t":"wr"#, 1);
    fs::write(&jp, text).unwrap();
    let r = recover(&h, &l).unwrap();
    assert!(matches!(r[0].outcome, RecoverOutcome::Corrupt { .. }));
    assert!(h.state_path().exists());
}

#[test]
fn rollback_after_a_completed_flip_restores_the_old_index() {
    let (_t, h, l) = setup();
    baseline(&h, &l, "0.1.0");
    let old = live_index(&h).unwrap();
    let mut t = Txn::begin(&h, "update", &l).unwrap();
    t.write(&h.state_path(), b"changed", None).unwrap();
    t.flip(FlipIntent {
        index: idx("0.2.0"),
    })
    .unwrap();
    assert_eq!(live_index(&h).unwrap(), idx("0.2.0"));
    let rep = t.rollback().unwrap();
    assert!(rep.is_complete());
    assert_eq!(live_index(&h).unwrap(), old);
}

#[test]
fn first_ever_flip_rolled_back_removes_the_index() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "install", &l).unwrap();
    t.flip(FlipIntent {
        index: idx("0.1.0"),
    })
    .unwrap();
    assert!(h.shim_index_path().exists());
    t.rollback().unwrap();
    assert!(!h.shim_index_path().exists());
}

#[test]
fn a_second_flip_and_an_oversized_index_are_refused() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "x", &l).unwrap();
    assert!(t
        .flip(FlipIntent {
            index: "x".repeat(65 * 1024)
        })
        .is_err());
    t.flip(FlipIntent {
        index: idx("0.1.0"),
    })
    .unwrap();
    let e = t
        .flip(FlipIntent {
            index: idx("0.2.0"),
        })
        .unwrap_err();
    assert_eq!(e.code(), "E1890");
}

#[test]
fn control_records_cannot_be_recorded_by_hand() {
    let (_t, h, l) = setup();
    let mut t = Txn::begin(&h, "x", &l).unwrap();
    assert!(t.record(JournalOp::Commit { recovered: false }).is_err());
    t.record(JournalOp::Mkdir {
        path: h.root().join("nothing"),
        before: Before::Absent,
    })
    .unwrap();
}

#[test]
fn prune_keeps_failures_and_young_commits() {
    let (_t, h, l) = setup();
    let t = Txn::begin(&h, "ok", &l).unwrap();
    let ok = t.id().to_string();
    t.commit().unwrap();
    let t2 = Txn::begin(&h, "interrupted", &l).unwrap();
    let open = t2.id().to_string();
    drop(t2);
    let infos = list_txns(&h);
    assert_eq!(infos.len(), 2);
    assert!(infos
        .iter()
        .any(|i| i.id == ok && i.status == TxnStatus::Committed));
    assert!(infos
        .iter()
        .any(|i| i.id == open && i.status == TxnStatus::Open));
    let day = std::time::Duration::from_secs(86_400);
    assert!(prune_txns(&h, &l, 7 * day).unwrap().is_empty());
    assert_eq!(
        prune_txns(&h, &l, std::time::Duration::ZERO).unwrap(),
        vec![ok]
    );
    assert!(Path::new(&h.txn_root().join(&open)).exists());
}

#[test]
fn txn_with_a_foreign_lock_is_refused() {
    let (_t, h, _l) = setup();
    let t2 = tempfile::tempdir().unwrap();
    let h2 = Home::at(t2.path().join(".cigscript"));
    let l2 = h2.lock().unwrap();
    assert!(Txn::begin(&h, "x", &l2).is_err());
}

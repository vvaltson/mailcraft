// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save Message As with `share` (a message dragged out of the window goes on to other people):
//! a draft's Bcc and the owner's own marks stay behind; without `share` the file is the owner's own.

use mailcraft_model::{Address, FlagStatus, Id};
use serde_json::json;

use crate::Session;

const NOW: i64 = 1_791_993_600;

fn saved(s: &mut Session, id: Id, share: bool) -> String {
    let r = s.run("file.exportMessage", &json!({"id": id, "share": share})).unwrap();
    let bytes = crate::cmd::base64_decode(r["data"].as_str().unwrap()).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A draft with a Bcc, a category and a flag, in the sample mailbox.
fn marked() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let id = *s.mailbox.messages.keys().next().unwrap();
    let m = s.mailbox.message_mut(id).unwrap();
    m.draft = true;
    m.bcc = vec![Address::new("Hidden", "hidden@example.org")];
    m.categories = vec!["Secret category".into()];
    m.flag = FlagStatus::Flagged;
    (s, id)
}

#[test]
fn a_shared_draft_keeps_its_bcc_and_marks_to_itself() {
    let (mut s, id) = marked();
    let shared = saved(&mut s, id, true);
    assert!(!shared.contains("hidden@example.org"), "no Bcc:\n{shared}");
    assert!(!shared.contains("Secret category"), "no category:\n{shared}");
}

#[test]
fn the_owners_own_save_keeps_the_draft_as_it_is() {
    let (mut s, id) = marked();
    assert!(saved(&mut s, id, false).contains("hidden@example.org"), "a draft saved by its owner keeps its Bcc");
}

#[test]
fn a_share_that_is_not_a_flag_is_the_owners_own_save() {
    let (mut s, id) = marked();
    let r = s.run("file.exportMessage", &json!({"id": id, "share": "yes"})).unwrap();
    let bytes = crate::cmd::base64_decode(r["data"].as_str().unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("hidden@example.org"));
}

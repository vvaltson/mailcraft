// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files dragged out of the window (#626): what the host is handed when the owner drags an
//! attachment or messages away, and what it is never handed.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::{Attachment, Id};

use crate::attachment_open::{drag_messages, start_drag};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

type Dragged = Arc<Mutex<Vec<Vec<(String, Vec<u8>)>>>>;

/// The sample mailbox, a host that notes each drag it is asked to start, and message `A` holding
/// the files named.
fn app(files: &[&str]) -> (MailApp, Dragged, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let id = *app.session.mailbox.messages.keys().next().unwrap();
    let attach = |name: &&str| Attachment::new(*name, "application/octet-stream", format!("content of {name}").into_bytes());
    app.session.mailbox.message_mut(id).unwrap().attachments = files.iter().map(attach).collect();
    let dragged: Dragged = Default::default();
    let sink = dragged.clone();
    app.services.drag_files = Some(Box::new(move |files| {
        sink.lock().unwrap().push(files.to_vec());
        Ok(())
    }));
    (app, dragged, id)
}

fn said(app: &MailApp) -> String {
    app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default()
}

#[test]
fn an_attachment_goes_out_under_its_own_name() {
    let (mut app, dragged, id) = app(&["report.pdf", "notes.txt"]);
    start_drag(&mut app, id, 1);
    assert_eq!(*dragged.lock().unwrap(), vec![vec![("notes.txt".to_string(), b"content of notes.txt".to_vec())]]);
    assert_eq!(said(&app), "");
}

#[test]
fn a_program_is_not_dragged_out() {
    let (mut app, dragged, id) = app(&["setup.exe"]);
    start_drag(&mut app, id, 0);
    assert!(dragged.lock().unwrap().is_empty(), "nothing handed to the system");
    assert!(!said(&app).is_empty(), "the refusal is said");
}

#[test]
fn hostile_attachment_choices_start_nothing() {
    let (mut app, dragged, id) = app(&["a.pdf"]);
    start_drag(&mut app, id, 1);
    start_drag(&mut app, id, usize::MAX);
    start_drag(&mut app, 0, 0);
    start_drag(&mut app, u64::MAX, 0);
    assert!(dragged.lock().unwrap().is_empty());
}

#[test]
fn no_host_drag_means_nothing_happens() {
    let (mut app, _, id) = app(&["a.pdf"]);
    app.services.drag_files = None;
    start_drag(&mut app, id, 0);
    assert_eq!(said(&app), "");
    assert!(drag_messages(&mut app, &[id]).is_ok());
}

#[test]
fn messages_go_out_as_eml_files_in_the_order_grabbed() {
    let (mut app, dragged, _) = app(&[]);
    let ids: Vec<Id> = app.session.mailbox.messages.keys().copied().take(3).collect();
    drag_messages(&mut app, &ids).unwrap();
    let dragged = dragged.lock().unwrap();
    assert_eq!(dragged.len(), 1, "one drag");
    assert_eq!(dragged[0].len(), 3);
    for (name, bytes) in &dragged[0] {
        assert!(name.ends_with(".eml"), "{name}");
        assert!(!name.contains(['/', '\\']), "{name}");
        assert!(String::from_utf8_lossy(bytes).contains("Subject:"), "{name} is a message");
    }
}

#[test]
fn a_selection_of_thousands_goes_out_in_part() {
    let (mut app, dragged, _) = app(&[]);
    let ids: Vec<Id> = app.session.mailbox.messages.keys().copied().cycle().take(5_000).collect();
    drag_messages(&mut app, &ids).unwrap();
    assert_eq!(dragged.lock().unwrap()[0].len(), 25);
}

#[test]
fn one_message_that_is_not_there_starts_no_drag() {
    let (mut app, dragged, id) = app(&[]);
    assert!(drag_messages(&mut app, &[id, 0]).is_err());
    assert!(drag_messages(&mut app, &[u64::MAX]).is_err());
    assert!(dragged.lock().unwrap().is_empty());
}

/// A window on the sample's Inbox, a host that notes each drag, and a message row on screen.
fn window() -> (egui_kittest::Harness<'static, MailApp>, Dragged, Id) {
    let mut h = crate::people_view::testing::window("view.mail");
    let dragged: Dragged = Default::default();
    let sink = dragged.clone();
    h.state_mut().services.drag_files = Some(Box::new(move |files| {
        sink.lock().unwrap().push(files.to_vec());
        Ok(())
    }));
    let id = h.state().session.mailbox.messages.keys().copied().find(|id| h.state().hit.rect("message", *id).is_some()).expect("a row on screen");
    (h, dragged, id)
}

/// Row `id` pressed and moved to `to` in steps, the button still down.
fn drag_row_to(h: &mut egui_kittest::Harness<'static, MailApp>, id: Id, to: egui::Pos2) {
    let start = h.state().hit.rect("message", id).unwrap().center();
    h.event(egui::Event::PointerMoved(start));
    h.step();
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.step();
    for k in 1..=8 {
        h.event(egui::Event::PointerMoved(start + (to - start) * (k as f32 / 8.0)));
        h.step();
    }
}

#[test]
fn a_row_dragged_out_of_the_window_goes_as_a_file_once() {
    let (mut h, dragged, id) = window();
    drag_row_to(&mut h, id, egui::pos2(-60.0, 300.0));
    h.run_steps(4);
    assert_eq!(dragged.lock().unwrap().len(), 1, "one drag, started once the pointer was out");
    assert_eq!(dragged.lock().unwrap()[0].len(), 1);
}

#[test]
fn a_row_dragged_inside_the_window_starts_no_system_drag() {
    let (mut h, dragged, id) = window();
    drag_row_to(&mut h, id, egui::pos2(100.0, 400.0));
    h.run_steps(4);
    assert!(dragged.lock().unwrap().is_empty());
}

// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files dragged out of the window (#626): what the host is handed when the owner drags an
//! attachment or messages away, and what it is never handed.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::{Attachment, Id};

use crate::attachment_open::{drag_attachment, drag_messages};
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
    drag_attachment(&mut app, id, 1).unwrap();
    assert_eq!(*dragged.lock().unwrap(), vec![vec![("notes.txt".to_string(), b"content of notes.txt".to_vec())]]);
    assert_eq!(said(&app), "");
}

#[test]
fn only_what_opens_without_a_question_goes_out() {
    for name in ["setup.exe", "macro.docm", "page.html", "drawing.svg", "README"] {
        let (mut app, dragged, id) = app(&[name]);
        assert!(drag_attachment(&mut app, id, 0).is_err(), "{name}");
        assert!(dragged.lock().unwrap().is_empty(), "{name}: nothing handed to the system");
    }
}

#[test]
fn hostile_attachment_choices_start_nothing() {
    let (mut app, dragged, id) = app(&["a.pdf"]);
    assert!(drag_attachment(&mut app, id, 1).is_err());
    assert!(drag_attachment(&mut app, id, usize::MAX).is_err());
    assert!(drag_attachment(&mut app, 0, 0).is_err());
    assert!(drag_attachment(&mut app, u64::MAX, 0).is_err());
    assert!(dragged.lock().unwrap().is_empty());
}

#[test]
fn no_host_drag_means_nothing_happens() {
    let (mut app, _, id) = app(&["a.pdf"]);
    app.services.drag_files = None;
    assert!(drag_attachment(&mut app, id, 0).is_ok());
    assert!(drag_messages(&mut app, &[id]).is_ok());
    assert_eq!(said(&app), "");
}

#[test]
fn a_host_that_cannot_drag_says_so_in_our_words() {
    let (mut app, _, id) = app(&["a.pdf"]);
    app.services.drag_files = Some(Box::new(|_| Err("no button press to start a drag for".into())));
    let e = drag_attachment(&mut app, id, 0).unwrap_err();
    assert!(!e.contains("button press"), "{e}");
}

#[test]
fn a_file_over_the_cap_is_refused_before_it_is_read() {
    let (mut app, dragged, id) = app(&["big.pdf"]);
    app.session.mailbox.message_mut(id).unwrap().attachments[0].data = Arc::new(vec![0u8; (128 << 20) + 1]);
    assert!(drag_attachment(&mut app, id, 0).is_err());
    assert!(drag_messages(&mut app, &[id]).is_err(), "messages too: by what they hold");
    assert!(dragged.lock().unwrap().is_empty());
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
fn twenty_five_messages_go_and_more_are_refused_not_cut() {
    let (mut app, dragged, _) = app(&[]);
    let all: Vec<Id> = app.session.mailbox.messages.keys().copied().cycle().take(26).collect();
    assert!(drag_messages(&mut app, &all[..25]).is_ok());
    assert_eq!(dragged.lock().unwrap()[0].len(), 25);
    assert!(drag_messages(&mut app, &all).is_err(), "26 are not cut to 25");
    assert_eq!(dragged.lock().unwrap().len(), 1, "nothing went of the 26");
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

#[test]
fn the_system_drag_leaves_egui_with_no_drag_under_way() {
    let (mut h, _, id) = window();
    drag_row_to(&mut h, id, egui::pos2(-60.0, 300.0));
    h.run_steps(4);
    assert_eq!(h.ctx.dragged_id(), None, "the release went to the system, egui must not wait for one");
}

#[test]
fn an_attachment_goes_out_when_the_pointer_leaves_the_window_and_not_before() {
    use crate::reading::attachment_key;
    for (to, goes) in [(egui::pos2(-60.0, 300.0), true), (egui::pos2(300.0, 300.0), false)] {
        let (mut h, dragged, _) = window();
        let id = 13;
        let m = h.state_mut().session.mailbox.message_mut(id).unwrap();
        m.attachments = vec![Attachment::new("report.pdf", "application/pdf", b"pdf".to_vec())];
        h.state_mut().run("mail.select", serde_json::json!({"id": id})).unwrap();
        h.run_steps(6);
        let start = h.state().hit.rect_in(0, "attachment", attachment_key(id, 0)).expect("the chip").center();
        h.event(egui::Event::PointerMoved(start));
        h.step();
        h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        for k in 1..=8 {
            h.event(egui::Event::PointerMoved(start + (to - start) * (k as f32 / 8.0)));
            h.step();
        }
        h.run_steps(4);
        assert_eq!(dragged.lock().unwrap().len(), usize::from(goes), "{to:?}");
    }
}

/// A calendar file dropped on the window is imported (the owner's doing); one of the files we wrote
/// for a drag, let go over the window again, is not.
#[test]
fn our_own_drag_let_go_over_the_window_is_not_a_drop() {
    let dir = std::env::temp_dir().join(format!("mailcraft-drag-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("meeting.ics");
    std::fs::write(&path, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//example.org//EN\r\nBEGIN:VEVENT\r\nUID:1@example.org\r\nDTSTAMP:20261101T000000Z\r\nDTSTART:20261112T100000Z\r\nDTEND:20261112T110000Z\r\nSUMMARY:Dropped meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").unwrap();
    let events = |staged: bool| {
        let (mut h, _, _) = window();
        let there = path.clone();
        h.state_mut().services.staged = Some(Box::new(move |p| staged && p == there));
        let before = h.state().session.mailbox.events.len();
        h.input_mut().dropped_files.push(std::sync::Arc::new(Dropped(path.clone())));
        h.run_steps(4);
        h.state().session.mailbox.events.len() - before
    };
    assert_eq!(events(false), 1, "the control: a dropped calendar file is imported");
    assert_eq!(events(true), 0, "our own file is not");
    let _ = std::fs::remove_dir_all(dir);
}

/// What the window's backend hands egui for a file let go over it.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

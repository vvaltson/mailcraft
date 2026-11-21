// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mail.openAttachment` in the window (#126): an attachment opened with the computer's own
//! program for that kind of file, as Outlook's double-click and Open do.
//!
//! What the sender chose is never trusted: the kind of file decides first
//! ([`mailcraft_engine::opening::verdict`]): programs, scripts, shortcuts, installers, disk
//! images and files of no kind are not opened (the owner can save them); Office files with macros
//! and web pages are asked about first ([`question`]). The host writes the file into a folder of
//! MailCraft's own, under a name made safe, and asks the system to open it
//! ([`Services::open_file`](crate::Services::open_file)). The browser has no such thing: there the
//! file is downloaded instead. Agents can't ask for any of it (`agent::access`).

use mailcraft_engine::opening::{self, Verdict};
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;

pub(crate) const OPEN: &str = "mail.openAttachment";

/// The attachment the params name: its message, its index and its name.
fn target(app: &MailApp, p: &Value) -> Result<(Id, usize, String), String> {
    // Read as the engine reads it (#553): named wrongly, it is refused, never taken for the selection.
    let id = mailcraft_engine::p::message_strict(p)
        .map_err(|e| e.to_string())?
        .or(app.session.sel.focus())
        .ok_or_else(|| tl!("Select a message first").to_string())?;
    let index = p.get("index").and_then(Value::as_u64).ok_or_else(|| "bad parameters: `index` (number) is required".to_string())?;
    let index = usize::try_from(index).map_err(|_| tl!("The message has no such attachment").to_string())?;
    let m = app.session.mailbox.message(id).ok_or_else(|| tl!("The message isn't there any more").to_string())?;
    let a = m.attachments.get(index).ok_or_else(|| tl!("The message has no such attachment").to_string())?;
    Ok((id, index, a.name.clone()))
}

/// What the owner is asked before an Office file with macros, a web page, Markdown, or a key,
/// certificate or signature is opened; `None`
/// for everything else (it opens, or it is refused, without a question).
pub(crate) fn question(app: &MailApp, p: &Value) -> Option<(String, String, String)> {
    let (_, _, name) = target(app, p).ok()?;
    let shown = opening::file_name(&name);
    let why = match opening::verdict(&name) {
        Verdict::Macros => tl!("Files like this can carry macros, which run as programs do. Open it only if you know who sent it and expect it."),
        Verdict::Scripts => {
            tl!("Pages like this open in the browser, where scripts in them run. Open it only if you know who sent it and expect it.")
        }
        Verdict::Active => tl!(
            "Programs that open files like this act on what is in them: they can show web content in it, or take in the keys and certificates it holds. Open it only if you know who sent it and expect it."
        ),
        Verdict::Open | Verdict::Blocked => return None,
    };
    let text = crate::i18n::fmt(tl!("Open {name}? {why}"), &[("name", &shown), ("why", why)]);
    Some((tl!("Opening Mail Attachment").to_string(), text, tl!("Open").to_string()))
}

/// What the owner is told when an attachment isn't opened on Linux or a BSD because neither
/// program that says what kind of file it is (`gio`, `kmimetypefinder`) is installed: the host
/// opens a file only once the desktop has said it is the kind its name says.
pub fn no_kind_teller_text() -> String {
    tl!("To open an attachment, MailCraft first asks the desktop what kind of file it is, and neither gio (from GLib) nor kmimetypefinder (from KDE) is installed. Install one of them, or save the attachment with Save As.").to_string()
}

/// What the owner is told when an attachment of a kind whose name has several types (a `.png`
/// can be two kinds of picture) isn't opened from the Flatpak sandbox: the computer outside types
/// it again, and may tell them apart by what is in the file.
pub fn sandbox_kind_text() -> String {
    tl!("This copy of MailCraft runs in a Flatpak sandbox, where it can't make sure that a file of this kind opens with the right program. Save it with Save As and open it yourself.").to_string()
}

/// `mail.openAttachment` from the window, the control channel or a test.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != OPEN {
        return None;
    }
    Some(open(app, p))
}

/// The most messages one drag out of the window carries, and the most bytes of them: a selection
/// of thousands is not written out file by file.
const MAX_DRAGGED: usize = 25;
const MAX_DRAGGED_MB: usize = 128;
const MAX_DRAGGED_BYTES: usize = MAX_DRAGGED_MB << 20;

/// An attachment grabbed in the reading pane. Like rows of the list it is carried inside the
/// window by egui, and goes on as a file only once the pointer has left the window with the
/// button down: a click that wobbles, or a drop back on the window, must not start anything.
pub(crate) struct AttachmentDrag {
    pub message: Id,
    pub index: usize,
}

/// The owner presses on attachment `index` of `message` and moves.
pub(crate) fn begin_drag(app: &MailApp, response: &egui::Response, message: Id, index: usize) {
    if app.services.drag_files.is_some() {
        response.dnd_set_drag_payload(AttachmentDrag { message, index });
    }
}

/// What is dragged from the window (an attachment, or messages from the list) reaches things
/// inside the window as before; once the pointer has left it with the button still down it goes on
/// as files (an attachment as Open writes it, a message as Save Message As writes it for sharing)
/// and the drag is the system's.
pub(crate) fn drag_out(app: &mut MailApp, ctx: &egui::Context) {
    if app.services.drag_files.is_none() {
        return;
    }
    let messages = egui::DragAndDrop::payload::<crate::mail_view::MessageDrag>(ctx);
    let attachment = egui::DragAndDrop::payload::<AttachmentDrag>(ctx);
    if messages.is_none() && attachment.is_none() {
        return;
    }
    // (The window's rect is read before `input`: a context call inside its closure can deadlock.)
    let window = ctx.content_rect();
    let outside = ctx.input(|i| i.pointer.primary_down() && i.pointer.latest_pos().is_none_or(|p| !window.contains(p)));
    if !outside {
        return;
    }
    egui::DragAndDrop::clear_payload(ctx);
    let handed = match (&messages, &attachment) {
        (Some(drag), _) => drag_messages(app, &drag.0),
        (None, Some(drag)) => drag_attachment(app, drag.message, drag.index),
        (None, None) => Ok(()),
    };
    // The system's drag takes the button's release: for egui the press is over.
    ctx.stop_dragging();
    if let Err(error) = handed {
        app.status(error);
    }
}

/// Attachment `index` of `message` for the system's drag: refused unless it is a kind that opens
/// without a question (what Open would ask about can't be asked in the middle of a drag).
pub(crate) fn drag_attachment(app: &mut MailApp, message: Id, index: usize) -> Result<(), String> {
    let (message, index, name) = target(app, &json!({"id": message, "index": index}))?;
    if opening::verdict(&name) != Verdict::Open {
        return Err(crate::i18n::fmt(
            tl!("{name} can't be dragged out: a file of this kind can carry a program or a script. Save it with Save As"),
            &[("name", &opening::file_name(&name).chars().take(60).collect::<String>())],
        ));
    }
    let size = app.session.mailbox.message(message).and_then(|m| m.attachments.get(index)).map_or(0, |a| a.size());
    if size > MAX_DRAGGED_BYTES {
        return Err(too_much());
    }
    let bytes = app.session.content(message, mailcraft_engine::Part::Attachment(index)).map_err(|e| e.to_string())?;
    hand_over(app, vec![(opening::file_name(&name), bytes.to_vec())])
}

/// `ids` as files for the system's drag, the one grabbed first leading. Nothing goes when there
/// are too many or too much, or when one of them can't be written (the answer says which and why).
pub(crate) fn drag_messages(app: &mut MailApp, ids: &[Id]) -> Result<(), String> {
    if ids.len() > MAX_DRAGGED {
        return Err(crate::i18n::fmt(tl!("Drag out {n} messages at most at a time"), &[("n", &MAX_DRAGGED.to_string())]));
    }
    // What the messages hold, by what the mailbox knows of them, before any is written out.
    let held = |id: &Id| {
        app.session.mailbox.message(*id).map_or(0, |m| {
            m.attachments
                .iter()
                .fold(m.body.text.len().saturating_add(m.body.html.as_ref().map_or(0, String::len)), |n, a| n.saturating_add(a.size()))
        })
    };
    if ids.iter().fold(0usize, |n, id| n.saturating_add(held(id))) > MAX_DRAGGED_BYTES {
        return Err(too_much());
    }
    let mut files = Vec::new();
    let mut total = 0usize;
    for id in ids {
        let saved = app.execute("file.exportMessage", json!({"id": id, "share": true}))?;
        let name = saved.get("name").and_then(Value::as_str).unwrap_or("message.eml").to_string();
        let data = saved.get("data").and_then(Value::as_str).ok_or("no message to drag")?;
        let bytes = mailcraft_engine::cmd::base64_decode(data).map_err(|e| e.to_string())?;
        total = total.saturating_add(bytes.len());
        if total > MAX_DRAGGED_BYTES {
            return Err(too_much());
        }
        files.push((name, bytes));
    }
    hand_over(app, files)
}

fn too_much() -> String {
    crate::i18n::fmt(tl!("That is more than {mb} MB to drag out at once: save it with Save As instead"), &[("mb", &MAX_DRAGGED_MB.to_string())])
}

/// `files` (name, content) to the host, which writes them and starts the system's drag.
fn hand_over(app: &MailApp, files: Vec<(String, Vec<u8>)>) -> Result<(), String> {
    match &app.services.drag_files {
        Some(drag) if !files.is_empty() => drag(&files).map_err(|e| {
            log::warn!("a drag out of the window didn't start: {e}");
            tl!("The drag couldn't be started").to_string()
        }),
        _ => Ok(()),
    }
}

fn open(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let (message, index, name) = target(app, p)?;
    let verdict = opening::verdict(&name);
    if verdict == Verdict::Blocked {
        return Err(opening::blocked_message(&name));
    }
    // The window asks first ([`question`]) and its yes says so; any other caller says so itself.
    if verdict.asks() && p.get("confirmed").and_then(Value::as_bool) != Some(true) {
        return Err("bad parameters: this kind of file is asked about first; `\"confirmed\": true` says the owner said yes".into());
    }
    let file = opening::file_name(&name);
    let bytes = app.session.content(message, mailcraft_engine::Part::Attachment(index)).map_err(|e| e.to_string())?;
    if let Some(open_file) = &app.services.open_file {
        open_file(&file, bytes.as_slice())?;
        app.status(crate::i18n::fmt(tl!("Opening {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "opened": true}));
    }
    // The browser opens nothing of the computer's: the file is downloaded instead.
    if let Some(download) = &app.services.download {
        download(&file, bytes.as_slice())?;
        app.status(crate::i18n::fmt(tl!("Downloaded {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "downloaded": true}));
    }
    Err(tl!("Attachments can't be opened here: save it with Save As instead.").to_string())
}

#[cfg(test)]
mod tests;

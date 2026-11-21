// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dragging a file out of the window on Windows (#626): the system's own drag (OLE), through the
//! `drag` crate, which keeps the native calls (this program forbids unsafe code).
//!
//! The window asks through `Services::drag_file`, which writes the file as Open does and
//! [`queue`]s it; the drag itself starts once the frame is drawn ([`start`]), because the system's
//! drag runs its own loop until the owner lets go.

use std::path::PathBuf;
use std::sync::Mutex;

static PENDING: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The file to be dragged when the frame is done.
pub fn queue(path: PathBuf) {
    if let Ok(mut pending) = PENDING.lock() {
        *pending = Some(path);
    }
}

/// Start the drag of the queued file, if there is one, from `window`. Returns when it is dropped
/// or called off. A failure is logged: the owner only sees the drag not start.
pub fn start(window: &eframe::Frame) {
    let Some(path) = PENDING.lock().ok().and_then(|mut pending| pending.take()) else { return };
    // The crate's own code isn't ours to make panic-free: a panic in it must not end the window.
    let item = drag::DragItem::Files(vec![path]);
    let ended = |result: drag::DragResult, _: drag::CursorPosition| log::debug!("a file dragged out of the window: {result:?}");
    let run = || drag::start_drag(window, item, drag::Image::Raw(Vec::new()), ended, drag::Options::default());
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::warn!("a file couldn't be dragged out of the window: {e}"),
        Err(_) => log::error!("a file couldn't be dragged out of the window: the system's drag failed"),
    }
}

// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dragging files out of the window under X11 (#626): the source side of XDND (freedesktop.org,
//! version 5), on a connection of our own. winit's X11 backend takes drops but can't start one,
//! and the button's grab is its connection's, so the pointer is followed by asking where it is
//! (`QueryPointer`) rather than by grabbing it, and the window under it is told with XDND's client
//! messages. The files go as a `text/uri-list` on the `XdndSelection`, which any X client may read
//! while the drag lasts, as with every X11 drag.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    self, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, KeyButMask, PropMode, SelectionNotifyEvent, SelectionRequestEvent, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndProxy,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        TARGETS,
        URI_LIST: b"text/uri-list",
    }
}

/// The XDND version spoken here; a window that speaks less than 3 is not dropped on.
const VERSION: u32 = 5;
/// How often the pointer is asked where it is.
const TICK: Duration = Duration::from_millis(15);
/// A drag held longer than this is given up.
const MAX_HELD: Duration = Duration::from_secs(300);
/// How long a window that said yes to the drop has to take the files and say it is done.
const AFTER_DROP: Duration = Duration::from_secs(10);
/// How long a window has to answer where the files may go.
const STATUS_WAIT: Duration = Duration::from_secs(1);
/// How deep the windows under the pointer are searched for one that takes drops.
const MAX_DEPTH: usize = 32;

/// One drag at a time: a second can't start while the button is still held for the first.
static DRAGGING: AtomicBool = AtomicBool::new(false);

/// Starts a drag of `paths` from where the pointer is, the button held: the connection and the
/// selection are taken here (an error reaches the window), the pointer is followed on a thread.
pub fn start(paths: &[PathBuf]) -> Result<(), String> {
    if DRAGGING.swap(true, Ordering::AcqRel) {
        return Err("a drag is already under way".into());
    }
    let begun = Drag::begin(uri_list(paths)).and_then(|drag| {
        std::thread::Builder::new()
            .name("x11-drag".into())
            .spawn(move || {
                if let Err(e) = drag.follow() {
                    log::warn!("X11 drag ended early: {e}");
                }
                DRAGGING.store(false, Ordering::Release);
            })
            .map(drop)
            .map_err(|e| e.to_string())
    });
    if begun.is_err() {
        DRAGGING.store(false, Ordering::Release);
    }
    begun
}

/// `paths` as XDND's `text/uri-list`: one `file://` URI a line, every byte but the unreserved and
/// `/` as `%XX`, each line ended by CR LF.
pub fn uri_list(paths: &[PathBuf]) -> Vec<u8> {
    let mut list = String::new();
    for path in paths {
        list.push_str(&uri(path));
        list.push_str("\r\n");
    }
    list.into_bytes()
}

fn uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut line = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            line.push(char::from(byte));
        } else {
            line.push_str(&format!("%{byte:02X}"));
        }
    }
    line
}

/// A window that takes drops: where its messages go (its proxy, when it names one) and the version
/// both sides speak.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Target {
    window: Window,
    to: Window,
    version: u32,
}

struct Drag {
    conn: RustConnection,
    atoms: Atoms,
    root: Window,
    /// Our own window, never shown: the source the targets answer and the selection's owner.
    source: Window,
    list: Vec<u8>,
}

impl Drag {
    fn begin(list: Vec<u8>) -> Result<Drag, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots.get(screen).ok_or("no such X screen")?.root;
        // The whole list goes in one property: a drag is at most a few dozen files.
        if list.len().saturating_add(64) > conn.maximum_request_bytes() {
            return Err("too many files for one drag".into());
        }
        let text = |e: x11rb::errors::ReplyOrIdError| e.to_string();
        let atoms = Atoms::new(&conn).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        let source = conn.generate_id().map_err(text)?;
        let aux = CreateWindowAux::new().override_redirect(1);
        conn.create_window(0, source, root, -100, -100, 1, 1, 0, WindowClass::INPUT_ONLY, x11rb::COPY_FROM_PARENT, &aux)
            .map_err(|e| e.to_string())?;
        conn.set_selection_owner(source, atoms.XdndSelection, x11rb::CURRENT_TIME).map_err(|e| e.to_string())?;
        let owner = conn.get_selection_owner(atoms.XdndSelection).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.owner;
        if owner != source {
            return Err("another program holds the drag".into());
        }
        Ok(Drag { conn, atoms, root, source, list })
    }

    /// Follows the pointer until the button is let go, then drops on the window under it if that
    /// window said yes, and serves it the files until it is done.
    fn follow(self) -> Result<(), x11rb::errors::ReplyError> {
        let held_since = Instant::now();
        let mut over: Option<Target> = None;
        // The last place sent to `over`, and when its answer was asked for (`None`: answered).
        let mut sent: Option<(i16, i16)> = None;
        let mut asked: Option<Instant> = None;
        let mut accepted = false;
        let mut dropped: Option<Instant> = None;
        loop {
            while let Some(event) = self.conn.poll_for_event()? {
                match event {
                    Event::SelectionRequest(request) if request.selection == self.atoms.XdndSelection => self.serve(&request)?,
                    Event::SelectionClear(clear) if clear.selection == self.atoms.XdndSelection => {
                        // Another drag took the selection: this one is over.
                        if let Some(target) = over.filter(|_| dropped.is_none()) {
                            self.tell(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                        }
                        return self.end();
                    }
                    Event::ClientMessage(message) => {
                        let [from, flags, ..] = message.data.as_data32();
                        if over.is_none_or(|t| t.window != from) {
                            continue;
                        }
                        if message.type_ == self.atoms.XdndStatus && dropped.is_none() {
                            accepted = flags & 1 == 1;
                            asked = None;
                        } else if message.type_ == self.atoms.XdndFinished && dropped.is_some() {
                            return self.end();
                        }
                    }
                    _ => {}
                }
            }
            if let Some(at) = dropped {
                if at.elapsed() > AFTER_DROP {
                    return self.end();
                }
            } else {
                if asked.is_some_and(|at| at.elapsed() > STATUS_WAIT) {
                    (asked, accepted) = (None, false);
                }
                let pointer = self.conn.query_pointer(self.root)?.reply()?;
                let held = pointer.mask.contains(KeyButMask::BUTTON1);
                if held {
                    let under = self.target_at();
                    if under != over {
                        if let Some(old) = over {
                            self.tell(old, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                        }
                        if let Some(new) = under {
                            self.tell(new, self.atoms.XdndEnter, [self.source, new.version << 24, self.atoms.URI_LIST, 0, 0])?;
                        }
                        (over, sent, asked, accepted) = (under, None, None, false);
                    }
                    let at = (pointer.root_x, pointer.root_y);
                    if let Some(target) = over
                        && asked.is_none()
                        && sent != Some(at)
                    {
                        let place = (u32::from(at.0.cast_unsigned()) << 16) | u32::from(at.1.cast_unsigned());
                        self.tell(target, self.atoms.XdndPosition, [self.source, 0, place, x11rb::CURRENT_TIME, self.atoms.XdndActionCopy])?;
                        (sent, asked) = (Some(at), Some(Instant::now()));
                    }
                    if held_since.elapsed() > MAX_HELD {
                        if let Some(target) = over {
                            self.tell(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                        }
                        return self.end();
                    }
                } else if asked.is_none() {
                    // Let go: dropped where a window said yes, else nothing happens.
                    match over {
                        Some(target) if accepted => {
                            self.tell(target, self.atoms.XdndDrop, [self.source, 0, x11rb::CURRENT_TIME, 0, 0])?;
                            dropped = Some(Instant::now());
                        }
                        Some(target) => {
                            self.tell(target, self.atoms.XdndLeave, [self.source, 0, 0, 0, 0])?;
                            return self.end();
                        }
                        None => return self.end(),
                    }
                }
            }
            self.conn.flush()?;
            std::thread::sleep(TICK);
        }
    }

    /// The window under the pointer that takes drops, searched from the top down; `None` over the
    /// desktop's bare root, or when a window goes away as it is asked.
    fn target_at(&self) -> Option<Target> {
        let mut window = self.root;
        for _ in 0..MAX_DEPTH {
            let child = self.conn.query_pointer(window).ok()?.reply().ok()?.child;
            if child == x11rb::NONE {
                return None;
            }
            if let Some(version) = self.aware(child) {
                return Some(Target { window: child, to: self.proxy(child).unwrap_or(child), version });
            }
            window = child;
        }
        None
    }

    /// The XDND version `window` takes drops with, when it takes them with 3 or later.
    fn aware(&self, window: Window) -> Option<u32> {
        let reply = self.conn.get_property(false, window, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1).ok()?.reply().ok()?;
        let version = reply.value32()?.next()?;
        (version >= 3).then_some(version.min(VERSION))
    }

    /// The window `window` has take its drops, when it names one that names itself (the spec's
    /// check that the proxy is not left over from a program gone).
    fn proxy(&self, window: Window) -> Option<Window> {
        let read = |w: Window| -> Option<Window> {
            let reply = self.conn.get_property(false, w, self.atoms.XdndProxy, AtomEnum::WINDOW, 0, 1).ok()?.reply().ok()?;
            reply.value32()?.next()
        };
        let proxy = read(window)?;
        (read(proxy) == Some(proxy)).then_some(proxy)
    }

    fn tell(&self, target: Target, kind: xproto::Atom, data: [u32; 5]) -> Result<(), x11rb::errors::ConnectionError> {
        let message = xproto::ClientMessageEvent::new(32, target.window, kind, data);
        self.conn.send_event(false, target.to, EventMask::NO_EVENT, message)?;
        Ok(())
    }

    /// Answers a window asking for the files (`text/uri-list`) or what they can be had as
    /// (`TARGETS`); anything else is refused.
    fn serve(&self, request: &SelectionRequestEvent) -> Result<(), x11rb::errors::ConnectionError> {
        // A requester from before ICCCM names no property: the target's name is used.
        let property = if request.property == x11rb::NONE { request.target } else { request.property };
        let answered = if request.target == self.atoms.URI_LIST {
            self.conn.change_property8(PropMode::REPLACE, request.requestor, property, self.atoms.URI_LIST, &self.list)?;
            true
        } else if request.target == self.atoms.TARGETS {
            let targets = [self.atoms.TARGETS, self.atoms.URI_LIST];
            self.conn.change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &targets)?;
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent {
            response_type: xproto::SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if answered { property } else { x11rb::NONE },
        };
        self.conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify)?;
        Ok(())
    }

    /// Our window goes, and the selection with it.
    fn end(self) -> Result<(), x11rb::errors::ReplyError> {
        self.conn.destroy_window(self.source)?;
        self.conn.flush()?;
        Ok(())
    }
}

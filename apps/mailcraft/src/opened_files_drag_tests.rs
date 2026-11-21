// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files written to be dragged out of the window (#626): the same staging and refusals as Open,
//! but writable, and recognised when one comes back to the window.

use std::fs;
use std::path::PathBuf;

use crate::opened_files::Opened;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-drag-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir.join("MailCraft").join("Attachments")
}

fn cleanup(base: &std::path::Path) {
    let _ = fs::remove_dir_all(base.parent().and_then(|p| p.parent()).unwrap_or(base));
}

#[test]
fn a_dragged_copy_is_writable_and_named_safely() {
    let base = scratch("write");
    let opened = Opened::at(base.clone());
    let path = opened.write_for_drag("../../evil/report.pdf", b"pdf").unwrap();
    assert!(path.starts_with(&base), "{path:?}");
    assert_eq!(path.file_name().unwrap(), "report.pdf");
    assert_eq!(fs::read(&path).unwrap(), b"pdf");
    assert!(!fs::metadata(&path).unwrap().permissions().readonly(), "what lands is the owner's to change");
    cleanup(&base);
}

#[test]
fn programs_and_unknown_files_are_not_staged_to_drag() {
    let base = scratch("refuse");
    let opened = Opened::at(base.clone());
    assert!(opened.write_for_drag("setup.exe", b"MZ").is_err());
    assert!(opened.write_for_drag("noextension", b"x").is_err());
    cleanup(&base);
}

#[test]
fn a_file_of_ours_is_recognised_and_one_of_the_owners_is_not() {
    let base = scratch("holds");
    let opened = Opened::at(base.clone());
    let ours = opened.write_for_drag("meeting.ics", b"x").unwrap();
    assert!(opened.holds(&ours));
    // Spelt another way round the same folder.
    let roundabout = ours.parent().unwrap().join("..").join(ours.parent().unwrap().file_name().unwrap()).join("meeting.ics");
    assert!(opened.holds(&roundabout), "{roundabout:?}");
    let theirs = base.parent().unwrap().join("meeting.ics");
    fs::write(&theirs, b"x").unwrap();
    assert!(!opened.holds(&theirs));
    assert!(!opened.holds(std::path::Path::new("/nonexistent/meeting.ics")));
    cleanup(&base);
}

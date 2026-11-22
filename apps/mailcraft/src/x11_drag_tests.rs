// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file list an X11 drag offers (#626): one `file://` URI a line, nothing in a name able to
//! end a line or start another URI.

use std::path::PathBuf;

use crate::x11_drag::uri_list;

#[test]
fn each_file_is_one_encoded_line() {
    let paths = [PathBuf::from("/tmp/MailCraft/1/report.pdf"), PathBuf::from("/tmp/MailCraft/2/Café notes #1.txt")];
    let list = String::from_utf8(uri_list(&paths)).unwrap();
    assert_eq!(list, "file:///tmp/MailCraft/1/report.pdf\r\nfile:///tmp/MailCraft/2/Caf%C3%A9%20notes%20%231.txt\r\n");
}

#[test]
fn a_name_cannot_add_a_line() {
    use std::os::unix::ffi::OsStrExt as _;
    let hostile = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/a\r\nfile:///etc/passwd\x00%"));
    let list = String::from_utf8(uri_list(&[hostile])).unwrap();
    assert_eq!(list.matches("\r\n").count(), 1, "{list}");
    assert!(list.ends_with("%0D%0Afile%3A///etc/passwd%00%25\r\n"), "{list}");
}

#[test]
fn no_files_is_an_empty_list() {
    assert!(uri_list(&[]).is_empty());
}

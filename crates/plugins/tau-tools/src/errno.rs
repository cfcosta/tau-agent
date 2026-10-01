//! Filesystem errors as Node prints them (`ENOENT: no such file or
//! directory, open '<path>'`), which is what models have seen from pi.

use std::{io, path::Path};

/// The errno name of `error`, such as `ENOENT`, or `EIO` when it has no
/// OS error code.
pub fn code(error: &io::Error) -> &'static str {
    code_and_text(error).0
}

fn code_and_text(error: &io::Error) -> (&'static str, &'static str) {
    // ENOENT, EACCES and EISDIR come from the kind below.
    match error.raw_os_error() {
        Some(1) => ("EPERM", "operation not permitted"),
        Some(20) => ("ENOTDIR", "not a directory"),
        Some(40) => ("ELOOP", "too many symbolic links encountered"),
        Some(36) => ("ENAMETOOLONG", "name too long"),
        Some(24) => ("EMFILE", "too many open files"),
        Some(28) => ("ENOSPC", "no space left on device"),
        Some(30) => ("EROFS", "read-only file system"),
        Some(17) => ("EEXIST", "file already exists"),
        _ => match error.kind() {
            io::ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
            io::ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
            io::ErrorKind::IsADirectory => {
                ("EISDIR", "illegal operation on a directory")
            }
            _ => ("EIO", "i/o error"),
        },
    }
}

/// Node's message for a failed `syscall` on `path`:
/// `<CODE>: <text>, <syscall> '<path>'`.
pub fn message(error: &io::Error, syscall: &str, path: &Path) -> String {
    let (code, text) = code_and_text(error);
    format!("{code}: {text}, {syscall} '{}'", path.display())
}

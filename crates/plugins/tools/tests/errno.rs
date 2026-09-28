//! Filesystem errors as Node prints them (`tau_tools::errno`).

use std::{io, path::Path};

use tau_tools::errno::{code, message};

/// Each OS error has Node's code and text; errors without an OS code
/// fall back by kind, and anything else is `EIO`.
#[test]
fn errors_read_as_node_prints_them() {
    let cases = [
        (2, "ENOENT", "no such file or directory"),
        (13, "EACCES", "permission denied"),
        (1, "EPERM", "operation not permitted"),
        (21, "EISDIR", "illegal operation on a directory"),
        (20, "ENOTDIR", "not a directory"),
        (40, "ELOOP", "too many symbolic links encountered"),
        (36, "ENAMETOOLONG", "name too long"),
        (24, "EMFILE", "too many open files"),
        (28, "ENOSPC", "no space left on device"),
        (30, "EROFS", "read-only file system"),
        (17, "EEXIST", "file already exists"),
    ];
    for (errno, name, text) in cases {
        let error = io::Error::from_raw_os_error(errno);
        assert_eq!(code(&error), name);
        assert_eq!(
            message(&error, "open", Path::new("/a b")),
            format!("{name}: {text}, open '/a b'")
        );
    }
    let kinds = [
        (io::ErrorKind::NotFound, "ENOENT: no such file or directory"),
        (io::ErrorKind::PermissionDenied, "EACCES: permission denied"),
        (
            io::ErrorKind::IsADirectory,
            "EISDIR: illegal operation on a directory",
        ),
        (io::ErrorKind::Other, "EIO: i/o error"),
    ];
    for (kind, expected) in kinds {
        let error = io::Error::from(kind);
        assert_eq!(
            message(&error, "read", Path::new("x")),
            format!("{expected}, read 'x'")
        );
    }
}

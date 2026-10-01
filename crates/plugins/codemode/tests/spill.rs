//! Where output too large for the model is written.

use std::os::unix::fs::PermissionsExt as _;

/// The spill file lives in `$TMPDIR`, which other users share, so only
/// its owner may read it.
#[test]
fn a_spill_file_is_readable_by_its_owner_only() {
    let path = tau_codemode::result::spill("secret output").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(text, "secret output");
}

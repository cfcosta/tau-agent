//! Path resolution (`tau_tools::path`), on a real directory
//! (`docs/reference/testing.md`, "tau-tools").

use std::path::PathBuf;

use hegel::{TestCase, generators as gs};
use tau_tools::path::Root;
use unicode_normalization::UnicodeNormalization;

/// How macOS may have written a name the model typed.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Variant {
    AmPm,
    Nfd,
    Curly,
    NfdCurly,
}

/// A screenshot-like name as typed: straight apostrophes, precomposed
/// accents, plain spaces, sometimes a time with `AM`/`pm` in any case.
#[hegel::composite]
fn typed_name(tc: &TestCase) -> String {
    let word = || gs::text().alphabet("abcéèç'").min_size(1).max_size(8);
    let mut name = tc.draw(word());
    if tc.draw(gs::booleans()) {
        let marker =
            tc.draw(gs::sampled_from(vec!["AM", "PM", "am", "pm", "Am"]));
        name.push_str(&format!(" 10.30.12 {marker}"));
    }
    name.push_str(".png");
    name
}

/// For a generated name, a file created under a macOS variant of it is
/// found from the typed form, as pi's `resolveReadPath` finds it.
#[hegel::test(test_cases = 50)]
fn macos_variants_are_found_from_the_typed_name(tc: TestCase) {
    let typed = tc.draw(typed_name());
    let variant = tc.draw(gs::sampled_from(vec![
        Variant::AmPm,
        Variant::Nfd,
        Variant::Curly,
        Variant::NfdCurly,
    ]));
    let curly = |s: &str| s.replace('\'', "\u{2019}");
    let on_disk = match variant {
        Variant::AmPm => {
            let mut s = typed.clone();
            for marker in [" AM.", " PM.", " am.", " pm.", " Am."] {
                s = s.replace(marker, &marker.replacen(' ', "\u{202F}", 1));
            }
            s
        }
        Variant::Nfd => typed.nfd().collect(),
        Variant::Curly => curly(&typed),
        Variant::NfdCurly => curly(&typed.nfd().collect::<String>()),
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(&on_disk), b"x").unwrap();
    let root = Root::new(dir.path());
    assert_eq!(
        root.resolve_read(&typed),
        dir.path().join(&on_disk),
        "{variant:?}"
    );
}

fn root() -> (tempfile::TempDir, Root) {
    let dir = tempfile::tempdir().unwrap();
    let root = Root::new(dir.path()).with_home("/home/someone");
    (dir, root)
}

/// `~` expands only on its own or before a `/`; `~draft.md` and
/// `@~draft.md` are literal file names (pi, `path-utils.test.ts:159`).
#[test]
fn tilde_expands_only_as_a_directory() {
    let (dir, root) = root();
    assert_eq!(root.resolve("~"), PathBuf::from("/home/someone"));
    assert_eq!(
        root.resolve("~/notes.md"),
        PathBuf::from("/home/someone/notes.md")
    );
    assert_eq!(root.resolve("~draft.md"), dir.path().join("~draft.md"));
    assert_eq!(root.resolve("@~draft.md"), dir.path().join("~draft.md"));
}

/// A leading `@` is stripped, unicode spaces become spaces, `file://`
/// URLs become paths, `.` and `..` are resolved, and an absolute path
/// ignores the root.
#[test]
fn inputs_are_normalized() {
    let (dir, root) = root();
    assert_eq!(root.resolve("@src/lib.rs"), dir.path().join("src/lib.rs"));
    assert_eq!(
        root.resolve("a\u{00A0}b\u{3000}c"),
        dir.path().join("a b c")
    );
    assert_eq!(
        root.resolve("file:///etc/hosts"),
        PathBuf::from("/etc/hosts")
    );
    assert_eq!(root.resolve("./a/../b/./c"), dir.path().join("b/c"));
    assert_eq!(root.resolve("/tmp/x"), PathBuf::from("/tmp/x"));
    assert_eq!(root.dir(), dir.path());
}

/// Lowercase `am`/`pm` are matched too (pi, `path-utils.test.ts:20`),
/// the exact name wins over a variant, and with nothing on disk the
/// resolved path comes back.
#[test]
fn read_resolution_known_cases() {
    let (dir, root) = root();
    let on_disk = "Screenshot 2024-01-01 at 9.41.00\u{202F}pm.png";
    std::fs::write(dir.path().join(on_disk), b"x").unwrap();
    assert_eq!(
        root.resolve_read("Screenshot 2024-01-01 at 9.41.00 pm.png"),
        dir.path().join(on_disk)
    );

    std::fs::write(dir.path().join("it's.png"), b"x").unwrap();
    std::fs::write(dir.path().join("it\u{2019}s.png"), b"x").unwrap();
    assert_eq!(root.resolve_read("it's.png"), dir.path().join("it's.png"));

    assert_eq!(
        root.resolve_read("missing.png"),
        dir.path().join("missing.png")
    );
}

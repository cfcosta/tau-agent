//! Path resolution (`tau_tools::path`), on a real directory
//! (`docs/reference/testing.md`, "tau-tools").

use std::path::{Component, PathBuf};

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
    // A name the variant leaves as typed (no marker, accent or
    // apostrophe) is found as itself, which says nothing about variants.
    tc.assume(on_disk != typed);
    tc.event(format!("{variant:?}"));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(&on_disk), b"x").unwrap();
    let root = Root::new(dir.path());
    assert_eq!(
        root.resolve_read(&typed),
        dir.path().join(&on_disk),
        "{variant:?}"
    );
}

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum PathPrefix {
    Relative,
    Absolute,
    AtRelative,
    AtAbsolute,
    TildeHome,
    HomeDirectory,
    FileUrlAbsolute,
    LiteralTildeWord,
}

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum SegmentKind {
    AsciiWord,
    CurrentDirectory,
    ParentDirectory,
    UnicodeSpaceWord,
}

#[hegel::composite]
fn path_prefix(tc: &TestCase) -> PathPrefix {
    tc.draw(gs::sampled_from(vec![
        PathPrefix::Relative,
        PathPrefix::Absolute,
        PathPrefix::AtRelative,
        PathPrefix::AtAbsolute,
        PathPrefix::TildeHome,
        PathPrefix::HomeDirectory,
        PathPrefix::FileUrlAbsolute,
        PathPrefix::LiteralTildeWord,
    ]))
}

#[hegel::composite]
fn path_segment(tc: &TestCase) -> (String, String) {
    match tc.draw(gs::sampled_from(vec![
        SegmentKind::AsciiWord,
        SegmentKind::CurrentDirectory,
        SegmentKind::ParentDirectory,
        SegmentKind::UnicodeSpaceWord,
    ])) {
        SegmentKind::AsciiWord => {
            let word = tc
                .draw(gs::text().alphabet("abcxyz012").min_size(1).max_size(6));
            (word.clone(), word)
        }
        SegmentKind::CurrentDirectory => (".".into(), ".".into()),
        SegmentKind::ParentDirectory => ("..".into(), "..".into()),
        SegmentKind::UnicodeSpaceWord => {
            let (source, expected) = tc.draw(gs::sampled_from(vec![
                ("a\u{00A0}b", "a b"),
                ("c\u{2000}d", "c d"),
                ("e\u{200A}f", "e f"),
                ("g\u{202F}h", "g h"),
                ("i\u{205F}j", "i j"),
                ("k\u{3000}l", "k l"),
            ]));
            (source.into(), expected.into())
        }
    }
}

#[hegel::composite]
fn path_segments(tc: &TestCase) -> Vec<(String, String)> {
    let mut segments: Vec<(String, String)> =
        tc.draw(gs::vecs(path_segment()).max_size(5));
    let file_word: String =
        tc.draw(gs::text().alphabet("abcxyz012").min_size(1).max_size(6));
    let file = format!("{file_word}.txt");
    segments.push((file.clone(), file));
    segments
}

fn raw_path(prefix: PathPrefix, segments: &[(String, String)]) -> String {
    let suffix = segments
        .iter()
        .map(|(source, _)| source.as_str())
        .collect::<Vec<_>>()
        .join("/");
    match prefix {
        PathPrefix::Relative => suffix,
        PathPrefix::Absolute => format!("/{suffix}"),
        PathPrefix::AtRelative => format!("@{suffix}"),
        PathPrefix::AtAbsolute => format!("@/{suffix}"),
        PathPrefix::TildeHome => format!("~/{suffix}"),
        PathPrefix::HomeDirectory => format!("/home/someone/{suffix}"),
        PathPrefix::FileUrlAbsolute => format!("file:///{suffix}"),
        PathPrefix::LiteralTildeWord => format!("~draft.md/{suffix}"),
    }
}

fn absolute_components(path: &str) -> Vec<String> {
    assert!(path.starts_with('/'));
    path.split('/')
        .filter(|component| !component.is_empty())
        .map(str::to_owned)
        .collect()
}

fn normalized_component(components: &mut Vec<String>, segment: &str) {
    // The description supplies the independently known component spelling.
    match segment {
        "" | "." => {}
        ".." => {
            components.pop();
        }
        _ => components.push(segment.to_owned()),
    }
}

fn modeled_path(
    root_path: &str,
    prefix: PathPrefix,
    segments: &[(String, String)],
) -> PathBuf {
    let mut components = match prefix {
        PathPrefix::Relative
        | PathPrefix::AtRelative
        | PathPrefix::LiteralTildeWord => absolute_components(root_path),
        PathPrefix::TildeHome | PathPrefix::HomeDirectory => {
            absolute_components("/home/someone")
        }
        PathPrefix::Absolute
        | PathPrefix::AtAbsolute
        | PathPrefix::FileUrlAbsolute => Vec::new(),
    };
    if matches!(prefix, PathPrefix::LiteralTildeWord) {
        normalized_component(&mut components, "~draft.md");
    }
    for (_, expected) in segments {
        normalized_component(&mut components, expected);
    }

    let mut path = PathBuf::from("/");
    for component in components {
        path.push(component);
    }
    path
}

/// Property inventory: `resolve` equals an independent absolute component
/// stack model, including prefix expansion, Unicode spaces, and dot segments.
/// Prefixes are selected directly; up to five generated segments precede a
/// guaranteed nonempty ASCII filename (six segments total), with no filtering.
/// Shrinking shortens the segment list and words while preserving that filename.
#[hegel::test(test_cases = 300)]
fn resolves_generated_paths_to_the_modeled_normalized_absolute_path(
    tc: TestCase,
) {
    let prefix = tc.draw(path_prefix());
    let segments = tc.draw(path_segments());
    let typed = raw_path(prefix, &segments);
    tc.event(format!("{prefix:?}"));

    let (_dir, root) = root();
    let expected =
        modeled_path(root.dir().to_str().unwrap(), prefix, &segments);
    let resolved = root.resolve(&typed);
    assert_eq!(resolved, expected, "{prefix:?}: {typed:?}");
    assert!(resolved.is_absolute(), "{typed:?} -> {resolved:?}");
    assert!(
        resolved.components().all(|component| !matches!(
            component,
            Component::CurDir | Component::ParentDir
        )),
        "{typed:?} -> {resolved:?}"
    );
    assert_eq!(
        root.resolve(resolved.to_str().unwrap()),
        resolved,
        "resolving {typed:?} twice"
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
    assert_eq!(
        root.resolve("~/../../tilde-root-boundary"),
        PathBuf::from("/tilde-root-boundary")
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
        root.resolve("a\u{2000}b\u{200A}c\u{202F}d\u{205F}e"),
        dir.path().join("a b c d e")
    );
    assert_eq!(
        root.resolve("file:///etc/hosts"),
        PathBuf::from("/etc/hosts")
    );
    assert_eq!(
        root.resolve("file:///../../url-root"),
        PathBuf::from("/url-root")
    );
    assert_eq!(root.resolve("./a/../b/./c"), dir.path().join("b/c"));
    assert_eq!(root.resolve("/tmp/x"), PathBuf::from("/tmp/x"));
    assert_eq!(
        root.resolve("/../../root-boundary"),
        PathBuf::from("/root-boundary")
    );
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

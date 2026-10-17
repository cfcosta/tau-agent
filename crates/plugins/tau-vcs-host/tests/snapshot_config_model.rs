//! A snapshot tracks what the jj CLI would track in the same workspace
//! (`docs/reference/vcs.md`, "Identity and jj config"), checked against a
//! model of jj's rule on made-up worlds of files and config:
//!
//! - a new file is tracked when no ignore pattern covers it or a folder
//!   above it, `snapshot.auto-track` matches it, and it is no larger than
//!   `snapshot.max-new-file-size` (0: no limit); one over the size is
//!   reported, with its size;
//! - the patterns come from Git's `core.excludesFile` (set in any Git
//!   config file or variable, `~/` for the home) or, when unset, the
//!   person's `git/ignore`, then the Git store's `info/exclude`, the last
//!   matching pattern winning;
//! - `snapshot.auto-track` is a fileset, with the person's
//!   `fileset-aliases`, a later file's alias replacing an earlier one's;
//! - a malformed `snapshot.auto-track` or `snapshot.max-new-file-size`
//!   is an error naming the setting, never a panic or a default;
//! - where the jj CLI of jj-lib's version is installed, it tracks and
//!   refuses the same files.
//!
//! The model never calls tau or jj. Each world lives in a temporary
//! directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{TestCase, generators as gs};
use tau_testing::block_on_io;
use tau_vcs_host::{Identity, JjEnv, Vcs};

const DIRS: [&str; 6] = ["", "src", "src/deep", "docs", "target", "src/target"];
const NAMES: [&str; 5] = ["a.rs", "b.txt", "keep.txt", "big.bin", "c.md"];

/// An ignore pattern, from a pool whose meaning the model knows.
const PATTERNS: [&str; 11] = [
    "*.txt",
    "*.bin",
    "docs/",
    "/target",
    "deep",
    "c.md",
    "!keep.txt",
    "!c.md",
    "!*.bin",
    "# a comment",
    "",
];

// What the world is.

/// A fileset, from a grammar whose meaning the model knows.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Fileset {
    All,
    None,
    /// `glob:"*.rs"`: `.rs` files at the top only.
    TopRs,
    /// `glob:"**/*.txt"`: `.txt` files anywhere.
    AnyTxt,
    /// `glob:"src/*"`: files right in `src`.
    InSrc,
    /// `root:"<dir>"`: the folder and everything under it.
    Under(usize),
    /// `root-file:"<path>"`: one file.
    File(usize, usize),
    Union(Box<Fileset>, Box<Fileset>),
    Intersection(Box<Fileset>, Box<Fileset>),
    Difference(Box<Fileset>, Box<Fileset>),
    Not(Box<Fileset>),
    /// `wanted`, the symbol alias.
    Wanted,
    /// `picked()`, the function alias.
    Picked,
}

/// How `snapshot.max-new-file-size` is written.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Size {
    Integer(u64),
    Text(u64),
    Bytes(u64),
    KiB(u64),
}

/// Where `core.excludesFile` is set.
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum GitFile {
    /// `~/.gitconfig`.
    Home,
    /// `git/config` in the person's config directory.
    Xdg,
    /// The Git store's own `config`.
    Repo,
    /// `GIT_CONFIG_COUNT` and its keys.
    Variables,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Excludes {
    set_in: GitFile,
    /// Written `~/…`, or as an absolute path.
    tilde: bool,
    patterns: Vec<usize>,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    /// Whether `XDG_CONFIG_HOME` is set (else `~/.config` is the
    /// config directory).
    xdg: bool,
    auto_track: Option<Fileset>,
    /// `wanted`'s and `picked()`'s definitions, in the person's
    /// `config.toml`, and again in a `conf.d` file that replaces them.
    aliases: (Fileset, Fileset),
    redefined: Option<(Fileset, Fileset)>,
    max_size: Option<Size>,
    excludes: Option<Excludes>,
    /// The person's `git/ignore`, read only when `core.excludesFile` is
    /// unset.
    default_ignore: Option<Vec<usize>>,
    info_exclude: Vec<usize>,
    /// The new files: a folder, a name, a size.
    files: Vec<((usize, usize), u64)>,
}

/// A fileset with no aliases in it: what an alias may stand for.
#[hegel::composite]
fn plain_fileset(tc: &TestCase, depth: u8) -> Fileset {
    fileset_of(tc, depth, false)
}

#[hegel::composite]
fn fileset(tc: &TestCase, depth: u8) -> Fileset {
    fileset_of(tc, depth, true)
}

fn fileset_of(tc: &TestCase, depth: u8, aliases: bool) -> Fileset {
    let leaves = if aliases { 9 } else { 7 };
    let kinds = if depth == 0 { leaves } else { leaves + 4 };
    let kind = tc.draw(gs::integers::<u8>().max_value(kinds - 1));
    let sub = |tc: &TestCase| -> Box<Fileset> {
        Box::new(if aliases {
            tc.draw(fileset(depth - 1))
        } else {
            tc.draw(plain_fileset(depth - 1))
        })
    };
    let leaf = |kind: u8| match kind {
        0 => Fileset::All,
        1 => Fileset::None,
        2 => Fileset::TopRs,
        3 => Fileset::AnyTxt,
        4 => Fileset::InSrc,
        5 => Fileset::Under(tc.draw(gs::integers().max_value(DIRS.len() - 1))),
        6 => Fileset::File(
            tc.draw(gs::integers().max_value(DIRS.len() - 1)),
            tc.draw(gs::integers().max_value(NAMES.len() - 1)),
        ),
        7 => Fileset::Wanted,
        _ => Fileset::Picked,
    };
    if kind < leaves {
        return leaf(kind);
    }
    match kind - leaves {
        0 => Fileset::Union(sub(tc), sub(tc)),
        1 => Fileset::Intersection(sub(tc), sub(tc)),
        2 => Fileset::Difference(sub(tc), sub(tc)),
        _ => Fileset::Not(sub(tc)),
    }
}

#[hegel::composite]
fn size(tc: &TestCase) -> Size {
    // Small limits, so the files around them stay small; 0, "no
    // limit", a quarter of the time.
    let limit = if tc.draw(gs::integers::<u8>().max_value(3)) == 0 {
        0
    } else {
        tc.draw(gs::integers::<u64>().min_value(1).max_value(48))
    };
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => Size::Integer(limit),
        1 => Size::Text(limit),
        2 => Size::Bytes(limit),
        _ => Size::KiB(tc.draw(gs::integers::<u64>().max_value(1))),
    }
}

fn patterns() -> impl hegel::generators::PrintableGenerator<Vec<usize>> {
    gs::vecs(gs::integers().max_value(PATTERNS.len() - 1)).max_size(5)
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    let max_size = tc.draw(gs::optional(size()));
    let limit = max_size.as_ref().map_or(u64::MAX, limit_of);
    let files: Vec<(usize, usize)> = tc.draw(
        gs::vecs(hegel::compose!(|tc| {
            (
                tc.draw(gs::integers().max_value(DIRS.len() - 1)),
                tc.draw(gs::integers().max_value(NAMES.len() - 1)),
            )
        }))
        .max_size(8)
        .unique(true),
    );
    let files = files
        .into_iter()
        .map(|file| {
            // Around the limit when there is a small one, else small.
            let size = if limit != 0 && limit < 4096 && tc.draw(gs::booleans())
            {
                let offset: u64 = tc.draw(gs::integers().max_value(4));
                (limit + offset).saturating_sub(2)
            } else {
                tc.draw(gs::integers().max_value(64))
            };
            (file, size)
        })
        .collect();
    World {
        xdg: tc.draw(gs::booleans()),
        auto_track: tc.draw(gs::optional(fileset(2))),
        aliases: (tc.draw(plain_fileset(1)), tc.draw(plain_fileset(1))),
        redefined: tc.draw(gs::optional(hegel::compose!(|tc| {
            (tc.draw(plain_fileset(1)), tc.draw(plain_fileset(1)))
        }))),
        max_size,
        excludes: tc.draw(gs::optional(hegel::compose!(|tc| {
            Excludes {
                set_in: tc.draw(gs::sampled_from(vec![
                    GitFile::Home,
                    GitFile::Xdg,
                    GitFile::Repo,
                    GitFile::Variables,
                ])),
                tilde: tc.draw(gs::booleans()),
                patterns: tc.draw(patterns()),
            }
        }))),
        default_ignore: tc.draw(gs::optional(patterns())),
        info_exclude: tc.draw(patterns()),
        files,
    }
}

// The model: jj's rule, written out.

/// What a snapshot tracks, and the new files it refuses, each with its
/// size and the limit it was over.
type Tracked = (BTreeSet<String>, BTreeMap<String, (u64, u64)>);

fn path_of((dir, name): (usize, usize)) -> String {
    match DIRS[dir] {
        "" => NAMES[name].to_owned(),
        dir => format!("{dir}/{}", NAMES[name]),
    }
}

/// The configured size limit, `u64::MAX` for none.
fn limit_of(size: &Size) -> u64 {
    let bytes = match size {
        Size::Integer(n) | Size::Text(n) | Size::Bytes(n) => *n,
        Size::KiB(k) => k * 1024,
    };
    if bytes == 0 { u64::MAX } else { bytes }
}

fn matches(world: &World, set: &Fileset, path: &str) -> bool {
    let (wanted, picked) = world.redefined.as_ref().unwrap_or(&world.aliases);
    match set {
        Fileset::All => true,
        Fileset::None => false,
        Fileset::TopRs => !path.contains('/') && path.ends_with(".rs"),
        Fileset::AnyTxt => path.ends_with(".txt"),
        Fileset::InSrc => path
            .strip_prefix("src/")
            .is_some_and(|rest| !rest.contains('/')),
        Fileset::Under(dir) => match DIRS[*dir] {
            "" => true,
            dir => path.starts_with(&format!("{dir}/")),
        },
        Fileset::File(dir, name) => path == path_of((*dir, *name)),
        Fileset::Union(a, b) => {
            matches(world, a, path) || matches(world, b, path)
        }
        Fileset::Intersection(a, b) => {
            matches(world, a, path) && matches(world, b, path)
        }
        Fileset::Difference(a, b) => {
            matches(world, a, path) && !matches(world, b, path)
        }
        Fileset::Not(a) => !matches(world, a, path),
        Fileset::Wanted => matches(world, wanted, path),
        Fileset::Picked => matches(world, picked, path),
    }
}

/// Whether `pattern` matches `path`, a folder when `dir`: `Some(true)`
/// to ignore, `Some(false)` to keep, `None` when it says nothing.
fn pattern_matches(pattern: &str, path: &str, dir: bool) -> Option<bool> {
    if pattern.is_empty() || pattern.starts_with('#') {
        return None;
    }
    let (keep, pattern) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let (dir_only, pattern) = match pattern.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    if dir_only && !dir {
        return None;
    }
    let (anchored, pattern) = match pattern.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let subject = if anchored {
        path
    } else {
        path.rsplit('/').next().unwrap()
    };
    let hit = match pattern.strip_prefix('*') {
        Some(suffix) => subject.ends_with(suffix),
        None => subject == pattern,
    };
    hit.then_some(!keep)
}

/// Whether the last pattern that says anything about `path` ignores it.
fn last_says(patterns: &[&str], path: &str, dir: bool) -> bool {
    patterns
        .iter()
        .rev()
        .find_map(|pattern| pattern_matches(pattern, path, dir))
        .unwrap_or(false)
}

/// Whether Git ignores the file at `path`: a folder above it ignored, or
/// the file itself.
fn ignored(world: &World, path: &str) -> bool {
    let base = match &world.excludes {
        Some(excludes) => Some(&excludes.patterns),
        None => world.default_ignore.as_ref(),
    };
    let patterns: Vec<&str> = base
        .into_iter()
        .flatten()
        .chain(&world.info_exclude)
        .map(|n| PATTERNS[*n])
        .collect();
    let parts: Vec<&str> = path.split('/').collect();
    for end in 1..parts.len() {
        if last_says(&patterns, &parts[..end].join("/"), true) {
            return true;
        }
    }
    last_says(&patterns, path, false)
}

/// What a snapshot of `world` tracks, and the new files it refuses for
/// their size.
fn expected(world: &World) -> Tracked {
    let limit = world.max_size.as_ref().map_or(1024 * 1024, limit_of);
    let mut tracked = BTreeSet::new();
    let mut too_large = BTreeMap::new();
    for (file, size) in &world.files {
        let path = path_of(*file);
        let auto = world.auto_track.as_ref().unwrap_or(&Fileset::All);
        if ignored(world, &path) || !matches(world, auto, &path) {
            continue;
        }
        if *size > limit {
            too_large.insert(path, (*size, limit));
        } else {
            tracked.insert(path);
        }
    }
    (tracked, too_large)
}

// The world on disk.

fn fileset_text(set: &Fileset) -> String {
    match set {
        Fileset::All => "all()".into(),
        Fileset::None => "none()".into(),
        Fileset::TopRs => "glob:\"*.rs\"".into(),
        Fileset::AnyTxt => "glob:\"**/*.txt\"".into(),
        Fileset::InSrc => "glob:\"src/*\"".into(),
        Fileset::Under(dir) => match DIRS[*dir] {
            "" => "root:\"\"".into(),
            dir => format!("root:{dir:?}"),
        },
        Fileset::File(dir, name) => {
            format!("root-file:{:?}", path_of((*dir, *name)))
        }
        Fileset::Union(a, b) => {
            format!("({} | {})", fileset_text(a), fileset_text(b))
        }
        Fileset::Intersection(a, b) => {
            format!("({} & {})", fileset_text(a), fileset_text(b))
        }
        Fileset::Difference(a, b) => {
            format!("({} ~ {})", fileset_text(a), fileset_text(b))
        }
        Fileset::Not(a) => format!("~{}", fileset_text(a)),
        Fileset::Wanted => "wanted".into(),
        Fileset::Picked => "picked()".into(),
    }
}

/// A TOML basic string: JSON's escapes are TOML's too.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

fn aliases_text((wanted, picked): &(Fileset, Fileset)) -> String {
    format!(
        "[fileset-aliases]\nwanted = {}\n'picked()' = {{ definition = {} }}\n",
        quoted(&fileset_text(wanted)),
        quoted(&fileset_text(picked)),
    )
}

fn size_text(size: &Size) -> String {
    match size {
        Size::Integer(n) => n.to_string(),
        Size::Text(n) => quoted(&n.to_string()),
        Size::Bytes(n) => quoted(&format!("{n}B")),
        Size::KiB(k) => quoted(&format!("{k}KiB")),
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn append(path: &Path, text: &str) {
    let mut old = std::fs::read_to_string(path).unwrap_or_default();
    old.push_str(text);
    write(path, &old);
}

fn ignore_text(patterns: &[usize]) -> String {
    patterns
        .iter()
        .map(|n| format!("{}\n", PATTERNS[*n]))
        .collect()
}

/// A world on disk: its person and its workspace.
struct Disk {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Disk {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn workspace(&self) -> PathBuf {
        self.root.join("work")
    }

    fn git_dir(&self) -> PathBuf {
        self.workspace().join(".jj/repo/store/git")
    }

    /// The person's config directory, where `XDG_CONFIG_HOME` points or
    /// `~/.config`.
    fn config_dir(&self, world: &World) -> PathBuf {
        if world.xdg {
            self.root.join("xdg")
        } else {
            self.home().join(".config")
        }
    }

    /// The environment the world's person runs in, tau's and the CLI's.
    fn vars(&self, world: &World) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        if world.xdg {
            vars.insert(
                "XDG_CONFIG_HOME".into(),
                self.config_dir(world).display().to_string(),
            );
        }
        if let Some(excludes) = &world.excludes
            && matches!(excludes.set_in, GitFile::Variables)
        {
            vars.insert("GIT_CONFIG_COUNT".into(), "1".into());
            vars.insert("GIT_CONFIG_KEY_0".into(), "core.excludesFile".into());
            vars.insert(
                "GIT_CONFIG_VALUE_0".into(),
                self.excludes_value(excludes),
            );
        }
        vars
    }

    fn identity(&self, world: &World) -> Identity {
        Identity {
            env: JjEnv {
                config_dir: Some(self.config_dir(world)),
                home_dir: Some(self.home()),
                system_dir: Some(self.root.join("etc")),
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars: self.vars(world),
            },
            ..Identity::default()
        }
    }

    fn excludes_path(&self, excludes: &Excludes) -> PathBuf {
        if excludes.tilde {
            self.home().join("my-ignore")
        } else {
            self.root.join("elsewhere").join("my-ignore")
        }
    }

    fn excludes_value(&self, excludes: &Excludes) -> String {
        if excludes.tilde {
            "~/my-ignore".into()
        } else {
            self.excludes_path(excludes).display().to_string()
        }
    }

    /// The person's jj config and Git's, written before the workspace
    /// is made.
    fn lay_out_config(&self, world: &World) {
        let jj = self.config_dir(world).join("jj");
        let mut text = aliases_text(&world.aliases);
        text.push_str("[snapshot]\n");
        if let Some(set) = &world.auto_track {
            text.push_str(&format!(
                "auto-track = {}\n",
                quoted(&fileset_text(set))
            ));
        }
        if let Some(size) = &world.max_size {
            text.push_str(&format!(
                "max-new-file-size = {}\n",
                size_text(size)
            ));
        }
        write(&jj.join("config.toml"), &text);
        if let Some(aliases) = &world.redefined {
            write(&jj.join("conf.d/later.toml"), &aliases_text(aliases));
        }
        let git = self.config_dir(world).join("git");
        if let Some(patterns) = &world.default_ignore {
            write(&git.join("ignore"), &ignore_text(patterns));
        }
        if let Some(excludes) = &world.excludes {
            write(
                &self.excludes_path(excludes),
                &ignore_text(&excludes.patterns),
            );
            let section = format!(
                "[core]\n\texcludesFile = {}\n",
                self.excludes_value(excludes)
            );
            match excludes.set_in {
                GitFile::Home => {
                    write(&self.home().join(".gitconfig"), &section)
                }
                GitFile::Xdg => write(&git.join("config"), &section),
                GitFile::Repo | GitFile::Variables => {}
            }
        }
    }

    /// What goes in the workspace once it exists: the Git store's own
    /// config and excludes, and the files.
    fn lay_out_workspace(&self, world: &World) {
        if let Some(excludes) = &world.excludes
            && matches!(excludes.set_in, GitFile::Repo)
        {
            append(
                &self.git_dir().join("config"),
                &format!(
                    "[core]\n\texcludesFile = {}\n",
                    self.excludes_value(excludes)
                ),
            );
        }
        write(
            &self.git_dir().join("info/exclude"),
            &ignore_text(&world.info_exclude),
        );
        for (file, size) in &world.files {
            let path = self.workspace().join(path_of(*file));
            write(&path, &"x".repeat(*size as usize));
        }
    }

    /// What tau's snapshot of the world tracks and refuses.
    fn tau(&self, world: &World) -> Result<Tracked, String> {
        self.lay_out_config(world);
        let identity = self.identity(world);
        let vcs = block_on_io(Vcs::init(self.workspace(), identity))
            .map_err(|error| error.to_string())?;
        self.lay_out_workspace(world);
        let copy = block_on_io(vcs.working_copy())
            .map_err(|error| error.to_string())?;
        Ok((
            copy.paths.into_iter().collect(),
            copy.too_large
                .into_iter()
                .map(|file| (file.path, (file.size, file.max_size)))
                .collect(),
        ))
    }
}

/// What a snapshot tracks is the model's: ignores, auto-track and size,
/// from every place jj reads them.
#[hegel::test(test_cases = 300)]
fn a_snapshot_tracks_what_jjs_rule_says(tc: TestCase) {
    let world = tc.draw(world());
    let disk = Disk::new();
    assert_eq!(disk.tau(&world).unwrap(), expected(&world));
}

/// The settings' bad values, each with the name the error must give.
const BAD: [(&str, &str); 8] = [
    ("auto-track = 'glob:\"[\"'", "snapshot.auto-track"),
    ("auto-track = 'nosuch()'", "snapshot.auto-track"),
    ("auto-track = 'all('", "snapshot.auto-track"),
    ("auto-track = 3", "snapshot.auto-track"),
    ("max-new-file-size = 'lots'", "snapshot.max-new-file-size"),
    ("max-new-file-size = -1", "snapshot.max-new-file-size"),
    ("max-new-file-size = true", "snapshot.max-new-file-size"),
    ("max-new-file-size = '12XB'", "snapshot.max-new-file-size"),
];

/// A malformed `snapshot.auto-track` or `snapshot.max-new-file-size` is
/// an error naming the setting, whatever else the world holds: never a
/// panic, never a default.
#[hegel::test(test_cases = 60)]
fn a_malformed_snapshot_setting_is_an_error_naming_it(tc: TestCase) {
    let world = tc.draw(world());
    let (line, name) = tc.draw(gs::sampled_from(BAD.to_vec()));
    let disk = Disk::new();
    disk.lay_out_config(&world);
    append(
        &disk.config_dir(&world).join("jj/conf.d/zz.toml"),
        &format!("[snapshot]\n{line}\n"),
    );
    let vcs = block_on_io(Vcs::init(disk.workspace(), disk.identity(&world)))
        .unwrap();
    disk.lay_out_workspace(&world);
    let error = block_on_io(vcs.working_copy()).unwrap_err().to_string();
    assert!(error.contains(name), "{line}: {error}");
    assert!(!error.contains("panicked"), "{line}: {error}");
}

/// Where the jj CLI of jj-lib's own version is installed, it tracks and
/// refuses the same files in the same world. Elsewhere this test says it
/// did not run.
#[hegel::test(test_cases = 40)]
fn the_jj_cli_tracks_the_same_files(tc: TestCase) {
    let Some(jj) = matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let world = tc.draw(world());
    let ours = Disk::new().tau(&world).unwrap();
    assert_eq!(ours, expected(&world));

    let disk = Disk::new();
    disk.lay_out_config(&world);
    let init = cli(&jj, &disk, &world)
        .args(["git", "init", "--no-colocate"])
        .arg(disk.workspace())
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    disk.lay_out_workspace(&world);
    let list = cli(&jj, &disk, &world)
        .current_dir(disk.workspace())
        .args(["file", "list"])
        .output()
        .unwrap();
    assert!(list.status.success(), "{list:?}");
    let tracked: BTreeSet<String> = String::from_utf8(list.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let refused: BTreeMap<String, (u64, u64)> = String::from_utf8(list.stderr)
        .unwrap()
        .lines()
        .filter_map(refused_line)
        .collect();
    assert_eq!((tracked, refused), ours);
}

/// `  <path>: <size> (<n> bytes); the maximum size allowed is <size>
/// (<m> bytes)`, the CLI's line for a file it refused.
fn refused_line(line: &str) -> Option<(String, (u64, u64))> {
    let (path, rest) = line.strip_prefix("  ")?.split_once(": ")?;
    let (_, rest) = rest.split_once(" (")?;
    let (bytes, rest) = rest.split_once(" bytes); the maximum size allowed")?;
    let (_, rest) = rest.split_once(" (")?;
    let max = rest.strip_suffix(" bytes)")?;
    Some((path.to_owned(), (bytes.parse().ok()?, max.parse().ok()?)))
}

/// tau writes a size as jj does, for every size.
#[hegel::test(test_cases = 300)]
fn sizes_read_as_jjs(tc: TestCase) {
    let bytes: u64 = if tc.draw(gs::booleans()) {
        tc.draw(gs::integers())
    } else {
        tc.draw(gs::integers().max_value(1 << 24))
    };
    assert_eq!(
        tau_vcs::byte_size(bytes),
        jj_lib::settings::HumanByteSize(bytes).to_string()
    );
}

/// The installed jj CLI, if its version is jj-lib's, by its full path:
/// the CLI runs with no `PATH`.
fn matching_jj() -> Option<PathBuf> {
    let jj = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("jj"))
        .find(|path| path.is_file())?;
    let output = Command::new(&jj).arg("--version").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    (version.trim() == "jj 0.45.1").then_some(jj)
}

/// The CLI in the world: its home, its variables, and no other variable
/// of this process.
fn cli(jj: &Path, disk: &Disk, world: &World) -> Command {
    let mut command = Command::new(jj);
    command
        .env_clear()
        .env("HOME", disk.home())
        .envs(disk.vars(world));
    command
}

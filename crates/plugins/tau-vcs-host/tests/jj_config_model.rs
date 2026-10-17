//! tau finds the person's jj config exactly as the jj CLI does
//! (`docs/reference/vcs.md`, "Identity and jj config"), checked against
//! a model of jj's rules on made-up worlds of config files:
//!
//! - every value comes from the last layer that sets it, in jj's order
//!   (defaults and tau's identity, the system's, the variables jj reads
//!   first, the person's or `JJ_CONFIG`'s, the repository's, the
//!   workspace's, the variables that override them all), each file's
//!   `--when` and `[[--scope]]` tables applied after it;
//! - a file jj never reads changes nothing, however broken;
//! - a file jj reads that is broken or unreadable is an error naming it;
//! - where the jj CLI of the same version is installed, it reads the
//!   same values.
//!
//! The model never calls tau: it is jj's documented precedence, written
//! out. Each world lives in a temporary directory and reads no real
//! config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{TestCase, generators as gs};
use jj_lib::secure_config::SecureConfig;
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use tau_vcs_host::{Identity, JjEnv, jj_settings};

/// The keys every world sets somewhere: who commits, and one that only
/// the environment's editor variables and the files set.
const KEYS: [&str; 3] = ["user.name", "user.email", "ui.editor"];

/// One config file's content: values at its top, an optional
/// file-wide `--when`, and `[[--scope]]` tables after them. Each
/// condition names either this world's workspace (it holds) or another
/// folder (it does not).
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Spec {
    when: Option<bool>,
    values: [Option<String>; 3],
    scopes: Vec<Scope>,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Scope {
    holds: bool,
    values: [Option<String>; 3],
}

/// An entry of `JJ_CONFIG`.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Entry {
    File(Spec),
    Dir(Vec<(String, Spec)>),
    /// A path to nothing: jj skips it.
    Missing,
    /// An empty item between separators: jj skips it.
    Empty,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    identity: (String, String),
    /// Whether the person has a config directory at all.
    config_dir: bool,
    system: Option<Spec>,
    system_d: Vec<(String, Spec)>,
    home: Option<Spec>,
    platform: Option<Spec>,
    conf_d: Vec<(String, Spec)>,
    jj_config: Option<Vec<Entry>>,
    repo: Option<Spec>,
    workspace: Option<Spec>,
    visual: Option<String>,
    editor: Option<String>,
    jj_user: Option<String>,
    jj_email: Option<String>,
    jj_editor: Option<String>,
}

/// A value any key could hold: printable text, quotes and backslashes
/// and other scripts included.
fn value() -> impl hegel::generators::PrintableGenerator<String> {
    gs::text().max_size(10).alphabet("ab Zé\"\\'#=[]{}.~ñ语👍")
}

#[hegel::composite]
fn values(tc: &TestCase) -> [Option<String>; 3] {
    [
        tc.draw(gs::optional(value())),
        tc.draw(gs::optional(value())),
        tc.draw(gs::optional(value())),
    ]
}

#[hegel::composite]
fn spec(tc: &TestCase) -> Spec {
    let scopes = tc.draw(
        gs::vecs(hegel::compose!(|tc| {
            Scope {
                holds: tc.draw(gs::booleans()),
                values: tc.draw(values()),
            }
        }))
        .max_size(2),
    );
    Spec {
        when: tc.draw(gs::optional(gs::booleans())),
        values: tc.draw(values()),
        scopes,
    }
}

/// Files of a directory, each `<stem>.toml`, stems unique.
#[hegel::composite]
fn dir(tc: &TestCase) -> Vec<(String, Spec)> {
    let stems: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-z0-9]{1,5}").fullmatch(true))
            .max_size(3)
            .unique(true),
    );
    stems
        .into_iter()
        .map(|stem| (stem, tc.draw(spec())))
        .collect()
}

#[hegel::composite]
fn entry(tc: &TestCase) -> Entry {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => Entry::File(tc.draw(spec())),
        1 => Entry::Dir(tc.draw(dir())),
        2 => Entry::Missing,
        _ => Entry::Empty,
    }
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    World {
        identity: (
            tc.draw(gs::from_regex("[a-z]{1,6}").fullmatch(true)),
            tc.draw(gs::from_regex("[a-z]{1,6}@tau").fullmatch(true)),
        ),
        config_dir: tc.draw(gs::booleans()),
        system: tc.draw(gs::optional(spec())),
        system_d: tc.draw(dir()),
        home: tc.draw(gs::optional(spec())),
        platform: tc.draw(gs::optional(spec())),
        conf_d: tc.draw(dir()),
        jj_config: tc.draw(gs::optional(gs::vecs(entry()).max_size(3))),
        repo: tc.draw(gs::optional(spec())),
        workspace: tc.draw(gs::optional(spec())),
        visual: tc.draw(gs::optional(value())),
        editor: tc.draw(gs::optional(value())),
        jj_user: tc.draw(gs::optional(value())),
        jj_email: tc.draw(gs::optional(value())),
        jj_editor: tc.draw(gs::optional(value())),
    }
}

// The model: jj's precedence, written out.

fn apply(spec: &Spec, into: &mut [Option<String>; 3]) {
    if spec.when == Some(false) {
        return;
    }
    let mut set = |values: &[Option<String>; 3]| {
        for (slot, value) in into.iter_mut().zip(values) {
            if let Some(value) = value {
                *slot = Some(value.clone());
            }
        }
    };
    set(&spec.values);
    for scope in spec.scopes.iter().filter(|scope| scope.holds) {
        set(&scope.values);
    }
}

fn apply_dir(files: &[(String, Spec)], into: &mut [Option<String>; 3]) {
    let mut files: Vec<&(String, Spec)> = files.iter().collect();
    // jj reads a directory's files in name order.
    files.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, spec) in files {
        apply(spec, into);
    }
}

fn expected(world: &World) -> [Option<String>; 3] {
    let mut values = [
        Some(world.identity.0.clone()),
        Some(world.identity.1.clone()),
        None,
    ];
    if world.jj_config.is_none() {
        if let Some(spec) = &world.system {
            apply(spec, &mut values);
        }
        apply_dir(&world.system_d, &mut values);
    }
    if let Some(editor) = world.visual.as_ref().or(world.editor.as_ref()) {
        values[2] = Some(editor.clone());
    }
    match &world.jj_config {
        Some(entries) => {
            for entry in entries {
                match entry {
                    Entry::File(spec) => apply(spec, &mut values),
                    Entry::Dir(files) => apply_dir(files, &mut values),
                    Entry::Missing | Entry::Empty => {}
                }
            }
        }
        None => {
            if let Some(spec) = &world.home {
                apply(spec, &mut values);
            }
            if world.config_dir {
                if let Some(spec) = &world.platform {
                    apply(spec, &mut values);
                }
                apply_dir(&world.conf_d, &mut values);
            }
        }
    }
    if world.config_dir {
        for spec in [&world.repo, &world.workspace].into_iter().flatten() {
            apply(spec, &mut values);
        }
    }
    for (slot, value) in values.iter_mut().zip([
        &world.jj_user,
        &world.jj_email,
        &world.jj_editor,
    ]) {
        if let Some(value) = value {
            *slot = Some(value.clone());
        }
    }
    values
}

// The world on disk.

/// A TOML basic string: JSON's escapes are TOML's too.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

fn assignments(values: &[Option<String>; 3]) -> String {
    KEYS.iter()
        .zip(values)
        .filter_map(|(key, value)| {
            value
                .as_ref()
                .map(|value| format!("{key} = {}\n", quoted(value)))
        })
        .collect()
}

/// A world on disk: where its person, its workspace and the folder no
/// condition matches are.
struct Disk {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Disk {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        Self { _dir: dir, root }
    }

    fn workspace(&self) -> PathBuf {
        self.root.join("work")
    }

    fn repo(&self) -> PathBuf {
        self.workspace().join(".jj").join("repo")
    }

    fn jj_dir(&self) -> PathBuf {
        self.root.join("config").join("jj")
    }

    fn text(&self, spec: &Spec) -> String {
        let path = |holds: bool| {
            let path = if holds {
                self.workspace()
            } else {
                self.root.join("elsewhere")
            };
            quoted(path.to_str().unwrap())
        };
        let mut text = String::new();
        if let Some(holds) = spec.when {
            text.push_str(&format!(
                "--when.repositories = [{}]\n",
                path(holds)
            ));
        }
        text.push_str(&assignments(&spec.values));
        for scope in &spec.scopes {
            text.push_str(&format!(
                "\n[[--scope]]\n--when.repositories = [{}]\n{}",
                path(scope.holds),
                assignments(&scope.values)
            ));
        }
        text
    }

    fn write(&self, path: &Path, spec: &Spec) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, self.text(spec)).unwrap();
    }

    fn write_dir(&self, dir: &Path, files: &[(String, Spec)]) {
        std::fs::create_dir_all(dir).unwrap();
        for (stem, spec) in files {
            self.write(&dir.join(format!("{stem}.toml")), spec);
        }
    }

    /// The config `jj config edit --repo` or `--workspace` would make,
    /// at the path jj gives it.
    fn write_secure(&self, secure: SecureConfig, kind: &str, spec: &Spec) {
        let mut rng = ChaCha20Rng::seed_from_u64(7);
        let loaded = secure
            .load_config(&mut rng, &self.jj_dir().join(kind))
            .unwrap();
        self.write(&loaded.config_file.unwrap(), spec);
    }

    /// Writes `world`, and returns the identity tau would run with in
    /// it.
    fn lay_out(&self, world: &World) -> Identity {
        std::fs::create_dir_all(self.repo()).unwrap();
        std::fs::create_dir_all(self.root.join("home")).unwrap();
        let etc = self.root.join("etc").join("jj");
        if let Some(spec) = &world.system {
            self.write(&etc.join("config.toml"), spec);
        }
        self.write_dir(&etc.join("conf.d"), &world.system_d);
        if let Some(spec) = &world.home {
            self.write(&self.root.join("home").join(".jjconfig.toml"), spec);
        }
        if let Some(spec) = &world.platform {
            self.write(&self.jj_dir().join("config.toml"), spec);
        }
        self.write_dir(&self.jj_dir().join("conf.d"), &world.conf_d);
        if let Some(spec) = &world.repo {
            self.write_secure(
                SecureConfig::new_repo(self.repo()),
                "repos",
                spec,
            );
        }
        if let Some(spec) = &world.workspace {
            self.write_secure(
                SecureConfig::new_workspace(self.workspace().join(".jj")),
                "workspaces",
                spec,
            );
        }
        let mut vars: HashMap<String, String> = HashMap::new();
        if let Some(entries) = &world.jj_config {
            let items: Vec<String> = entries
                .iter()
                .enumerate()
                .map(|(n, entry)| {
                    let path = self.root.join("jj_config").join(n.to_string());
                    match entry {
                        Entry::File(spec) => {
                            let path = path.with_extension("toml");
                            self.write(&path, spec);
                            path.display().to_string()
                        }
                        Entry::Dir(files) => {
                            self.write_dir(&path, files);
                            path.display().to_string()
                        }
                        Entry::Missing => path.display().to_string(),
                        Entry::Empty => String::new(),
                    }
                })
                .collect();
            vars.insert("JJ_CONFIG".into(), items.join(":"));
        }
        for (name, value) in [
            ("VISUAL", &world.visual),
            ("EDITOR", &world.editor),
            ("JJ_USER", &world.jj_user),
            ("JJ_EMAIL", &world.jj_email),
            ("JJ_EDITOR", &world.jj_editor),
        ] {
            if let Some(value) = value {
                vars.insert(name.into(), value.clone());
            }
        }
        Identity {
            name: world.identity.0.clone(),
            email: world.identity.1.clone(),
            env: JjEnv {
                config_dir: world.config_dir.then(|| self.root.join("config")),
                home_dir: Some(self.root.join("home")),
                system_dir: Some(self.root.join("etc")),
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars,
            },
        }
    }

    fn resolve(
        &self,
        identity: &Identity,
    ) -> Result<[Option<String>; 3], String> {
        let settings = jj_settings(identity, &self.workspace(), &self.repo())
            .map_err(|error| error.to_string())?;
        Ok(KEYS.map(|key| settings.get_string(key).ok()))
    }
}

/// Every key holds what the last layer setting it says, in jj's order.
#[hegel::test(test_cases = 300)]
fn every_value_comes_from_the_last_layer_that_sets_it(tc: TestCase) {
    let world = tc.draw(world());
    let disk = Disk::new();
    let identity = disk.lay_out(&world);
    assert_eq!(disk.resolve(&identity).unwrap(), expected(&world));
}

/// Broken files where jj never looks: not `.toml`, a folder inside
/// `conf.d`, the default files while `JJ_CONFIG` names the config (the
/// system's too), the person's directory's files when they have none.
const BROKEN: &str = "this is [not toml";

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Unread {
    NotToml,
    Subfolder,
    DefaultsUnderJjConfig,
    SystemUnderJjConfig,
    NoConfigDir,
}

/// A file jj never reads changes nothing, however broken it is.
#[hegel::test(test_cases = 150)]
fn a_file_jj_never_reads_changes_nothing(tc: TestCase) {
    let mut world = tc.draw(world());
    let unread = tc.draw(gs::sampled_from(vec![
        Unread::NotToml,
        Unread::Subfolder,
        Unread::DefaultsUnderJjConfig,
        Unread::SystemUnderJjConfig,
        Unread::NoConfigDir,
    ]));
    match unread {
        Unread::DefaultsUnderJjConfig | Unread::SystemUnderJjConfig => {
            world.jj_config.get_or_insert_with(Vec::new);
        }
        Unread::NoConfigDir => world.config_dir = false,
        Unread::NotToml | Unread::Subfolder => {}
    }
    let disk = Disk::new();
    let identity = disk.lay_out(&world);
    let before = disk.resolve(&identity).unwrap();
    let conf_d = disk.jj_dir().join("conf.d");
    let path = match unread {
        Unread::NotToml => conf_d.join(tc.draw(gs::sampled_from(vec![
            "notes.txt",
            "config.toml.bak",
            "toml",
            ".toml.swp",
        ]))),
        Unread::Subfolder => conf_d.join("nested").join("deep.toml"),
        Unread::DefaultsUnderJjConfig => disk.jj_dir().join("config.toml"),
        Unread::SystemUnderJjConfig => {
            disk.root.join("etc").join("jj").join("config.toml")
        }
        Unread::NoConfigDir => disk.jj_dir().join("config.toml"),
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, BROKEN).unwrap();
    assert_eq!(disk.resolve(&identity).unwrap(), before, "{path:?}");
    assert_eq!(before, expected(&world));
}

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Read {
    Home,
    Platform,
    ConfD,
    System,
    SystemD,
    JjConfigFile,
    JjConfigDir,
    Repo,
}

#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Fault {
    Malformed,
    Unreadable,
    BadCondition,
}

/// A file jj reads that is malformed, unreadable or carries a condition
/// jj cannot read is an error naming that file, never a skip and never
/// a panic.
#[hegel::test(test_cases = 150)]
fn a_broken_file_jj_reads_is_an_error_naming_it(tc: TestCase) {
    let world = World {
        jj_config: None,
        config_dir: true,
        ..tc.draw(world())
    };
    let read = tc.draw(gs::sampled_from(vec![
        Read::Home,
        Read::Platform,
        Read::ConfD,
        Read::System,
        Read::SystemD,
        Read::JjConfigFile,
        Read::JjConfigDir,
        Read::Repo,
    ]));
    let fault = tc.draw(gs::sampled_from(vec![
        Fault::Malformed,
        Fault::Unreadable,
        Fault::BadCondition,
    ]));
    let disk = Disk::new();
    let mut identity = disk.lay_out(&world);
    let jj_config = disk.root.join("jj_config");
    let path = match read {
        Read::Home => disk.root.join("home").join(".jjconfig.toml"),
        Read::Platform => disk.jj_dir().join("config.toml"),
        Read::ConfD => disk.jj_dir().join("conf.d").join("zz.toml"),
        Read::System => disk.root.join("etc/jj/config.toml"),
        Read::SystemD => disk.root.join("etc/jj/conf.d/zz.toml"),
        Read::JjConfigFile => jj_config.join("only.toml"),
        Read::JjConfigDir => jj_config.join("dir").join("zz.toml"),
        Read::Repo => {
            let mut rng = ChaCha20Rng::seed_from_u64(7);
            SecureConfig::new_repo(disk.repo())
                .load_config(&mut rng, &disk.jj_dir().join("repos"))
                .unwrap()
                .config_file
                .unwrap()
        }
    };
    match read {
        Read::JjConfigFile => {
            identity
                .env
                .vars
                .insert("JJ_CONFIG".into(), path.display().to_string());
        }
        Read::JjConfigDir => {
            identity.env.vars.insert(
                "JJ_CONFIG".into(),
                path.parent().unwrap().display().to_string(),
            );
        }
        _ => {}
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    match fault {
        Fault::Malformed => std::fs::write(&path, BROKEN).unwrap(),
        Fault::BadCondition => {
            std::fs::write(
                &path,
                "--when.repositories = 3\nuser.name = \"x\"\n",
            )
            .unwrap();
        }
        Fault::Unreadable => {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::write(&path, "user.name = \"x\"\n").unwrap();
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(0o000),
            )
            .unwrap();
        }
    }
    let error = disk.resolve(&identity).unwrap_err();
    // A condition is read once every file is: its error says what is
    // wrong with it, not where it came from.
    if !matches!(fault, Fault::BadCondition) {
        assert!(
            error.contains(&path.display().to_string()),
            "{read:?} {fault:?}: {error}"
        );
    }
}

/// Where the jj CLI of jj-lib's own version is installed, it reads every
/// world the way tau does. The system's files are left out: the CLI
/// reads the real `/etc`. Elsewhere this test says it did not run.
#[hegel::test(test_cases = 40)]
fn the_jj_cli_reads_the_same_values(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let world = World {
        system: None,
        system_d: Vec::new(),
        // The CLI finds the person's directory from `$XDG_CONFIG_HOME`.
        config_dir: true,
        ..tc.draw(world())
    };
    let disk = Disk::new();
    let identity = disk.lay_out(&world);
    // A real repository, for the CLI to load.
    std::fs::remove_dir_all(disk.workspace().join(".jj")).unwrap();
    let init = cli(&jj, &disk, &identity.env.vars)
        .args(["git", "init"])
        .arg(disk.workspace())
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let identity = disk.lay_out(&world);
    let ours = disk.resolve(&identity).unwrap();
    for ((key, ours), model) in KEYS.iter().zip(&ours).zip(expected(&world)) {
        assert_eq!(ours, &model, "{key}");
        let theirs = cli(&jj, &disk, &identity.env.vars)
            .current_dir(disk.workspace())
            .args(["config", "get", key])
            .output()
            .unwrap();
        // tau's identity and an unset editor are tau's own: the CLI
        // falls back to its defaults there.
        let Some(ours) = ours else { continue };
        if world_fell_back(&world, key) {
            continue;
        }
        assert!(theirs.status.success(), "{key}: {theirs:?}");
        let theirs = String::from_utf8(theirs.stdout).unwrap();
        assert_eq!(theirs.strip_suffix('\n').unwrap_or(&theirs), ours, "{key}");
    }
}

/// Whether nothing in `world` set `key`, so its value is tau's fallback.
fn world_fell_back(world: &World, key: &str) -> bool {
    let index = KEYS.iter().position(|k| *k == key).unwrap();
    let unset = World {
        identity: ("\u{0}".into(), "\u{0}".into()),
        ..world.clone()
    };
    expected(&unset)[index].as_deref() == Some("\u{0}")
}

/// The CLI in `disk`'s world: its home and config directory, and no
/// other variable of this process.
fn cli(jj: &Path, disk: &Disk, vars: &HashMap<String, String>) -> Command {
    let mut command = Command::new(jj);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", disk.root.join("home"))
        .env("XDG_CONFIG_HOME", disk.root.join("config"))
        .env("JJ_RANDOMNESS_SEED", "7")
        .envs(vars);
    command
}

/// The person's own files, each naming a different author, with nothing
/// above them: only their order decides who wins, so a change to that
/// order cannot hide behind the layers that usually cover it.
#[hegel::test(test_cases = 200)]
fn the_persons_files_layer_in_jjs_order(tc: TestCase) {
    let label = |tag: &str| Spec {
        when: None,
        values: [Some(tag.to_owned()), None, None],
        scopes: Vec::new(),
    };
    let stems: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-z0-9]{1,5}").fullmatch(true))
            .max_size(4)
            .unique(true),
    );
    let files: Vec<(String, Spec)> = stems
        .iter()
        .map(|stem| (stem.clone(), label(&format!("conf.d/{stem}"))))
        .collect();
    let jj_config = tc.draw(gs::booleans()).then(|| {
        // Its entries, in an order of their own, each a file or a
        // directory of the files above.
        let count: usize = tc.draw(gs::integers().max_value(4));
        (0..count)
            .map(|n| {
                if tc.draw(gs::booleans()) {
                    Entry::File(label(&format!("JJ_CONFIG/{n}")))
                } else {
                    Entry::Dir(files.clone())
                }
            })
            .collect::<Vec<_>>()
    });
    let world = World {
        identity: ("tau".into(), "tau@tau".into()),
        config_dir: tc.draw(gs::booleans()),
        system: tc.draw(gs::booleans()).then(|| label("system")),
        system_d: Vec::new(),
        home: tc.draw(gs::booleans()).then(|| label("home")),
        platform: tc.draw(gs::booleans()).then(|| label("platform")),
        conf_d: files,
        jj_config,
        repo: None,
        workspace: None,
        visual: None,
        editor: None,
        jj_user: None,
        jj_email: None,
        jj_editor: None,
    };
    let disk = Disk::new();
    let identity = disk.lay_out(&world);
    assert_eq!(disk.resolve(&identity).unwrap()[0], expected(&world)[0]);
}

/// What a path setting holds, for [`a_path_settings_home_is_the_environments`].
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum PathValue {
    /// `~/<rest>`: the home directory's `<rest>`.
    Home(String),
    /// `~<rest>`, no slash: another user's home, which jj leaves as is.
    OtherUser(String),
    Absolute(String),
    Relative(String),
}

impl PathValue {
    fn text(&self) -> String {
        match self {
            Self::Home(rest) => format!("~/{rest}"),
            Self::OtherUser(rest) => format!("~{rest}"),
            Self::Absolute(rest) => format!("/{rest}"),
            Self::Relative(rest) => rest.clone(),
        }
    }
}

/// `~/` at the start of a path setting jj-lib reads (`signing.key`, the
/// SSH backend's `allowed-signers` and `revocation-list`) is the
/// environment's home, never this process's, as jj 0.46 expands it
/// with its config environment's home (`UserSettings::get_path`); any
/// other value is the path as written. Without a home, `~/` stays.
#[hegel::test(test_cases = 100)]
fn a_path_settings_home_is_the_environments(tc: TestCase) {
    let rest = || {
        gs::from_regex("[a-z0-9._-]{1,6}(/[a-z0-9._-]{1,6}){0,2}")
            .fullmatch(true)
    };
    let value = match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => PathValue::Home(tc.draw(rest())),
        1 => PathValue::OtherUser(tc.draw(rest())),
        2 => PathValue::Absolute(tc.draw(rest())),
        _ => PathValue::Relative(tc.draw(rest())),
    };
    let key = tc.draw(gs::sampled_from(vec![
        "signing.key",
        "signing.backends.ssh.allowed-signers",
        "signing.backends.ssh.revocation-list",
    ]));
    let has_home = tc.draw(gs::booleans());
    let disk = Disk::new();
    let world = World {
        jj_config: None,
        config_dir: true,
        ..tc.draw(world())
    };
    let mut identity = disk.lay_out(&world);
    let file = disk.root.join("person.toml");
    std::fs::write(&file, format!("{key} = {}\n", quoted(&value.text())))
        .unwrap();
    identity
        .env
        .vars
        .insert("JJ_CONFIG".into(), file.display().to_string());
    if !has_home {
        identity.env.home_dir = None;
    }
    let settings =
        jj_settings(&identity, &disk.workspace(), &disk.repo()).unwrap();
    // The model: jj's `expand_home_path`.
    let expected = match (&value, &identity.env.home_dir) {
        (PathValue::Home(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(value.text()),
    };
    assert_eq!(settings.get_path(key).unwrap(), expected);
}

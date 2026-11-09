//! Where tau's own packages are (ADR 0051): the folder `plugins/`, which
//! is not in the binary. It is here, not in the host half, as the demo
//! reads fixtures from it and links no Luau.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// What names the folder over anything else.
pub const OVERRIDE: &str = "TAU_PLUGINS_DIR";

/// The folder from the directory the executable is in, installed.
pub const INSTALLED: &str = "../share/tau/plugins";

/// The folder in the source tree this was built from: for `cargo run`, a
/// binary started from `target/`, and tests.
pub const SOURCE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../../plugins");

/// The folder of tau's packages. It only looks: call it from
/// `spawn_blocking` where blocking matters.
pub fn find() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().ok();
    find_from(
        std::env::var_os(OVERRIDE),
        exe.as_deref(),
        Path::new(SOURCE),
    )
}

/// [`find`], given what it reads: the override wins and never falls
/// through, then the installed folder beside `exe`, then `source`.
pub fn find_from(
    wanted: Option<OsString>,
    exe: Option<&Path>,
    source: &Path,
) -> Result<PathBuf, String> {
    if let Some(wanted) = wanted.filter(|wanted| !wanted.is_empty()) {
        let wanted = PathBuf::from(wanted);
        return if wanted.is_dir() {
            Ok(wanted)
        } else {
            Err(format!(
                "tau's plugins are not at {}, which {OVERRIDE} names",
                wanted.display()
            ))
        };
    }
    let installed = exe.and_then(Path::parent).map(|dir| dir.join(INSTALLED));
    if let Some(installed) = installed.as_deref().filter(|dir| dir.is_dir()) {
        return Ok(installed
            .canonicalize()
            .unwrap_or_else(|_| installed.to_owned()));
    }
    if source.is_dir() {
        return Ok(source.canonicalize().unwrap_or_else(|_| source.to_owned()));
    }
    let tried = installed
        .iter()
        .map(|dir| dir.display().to_string())
        .chain([source.display().to_string()])
        .collect::<Vec<_>>()
        .join(" or ");
    Err(format!(
        "tau's plugins are not at {tried}: install them there, or name their \
         folder with {OVERRIDE}"
    ))
}

/// The fixture `name` (`fixtures/<name>.json`) of the package `plugin`, for
/// the demo and tests. It blocks.
pub fn fixture(plugin: &str, name: &str) -> Result<String, String> {
    let path = find()?
        .join(plugin)
        .join("fixtures")
        .join(format!("{name}.json"));
    std::fs::read_to_string(&path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

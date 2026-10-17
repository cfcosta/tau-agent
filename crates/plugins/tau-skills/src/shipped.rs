//! The skills tau-skills ships, from the crate's `skills/` folder:
//! engineering principles, playbooks and procedures, adapted from
//! pstack (see THIRD_PARTY_NOTICES.md). They are installed under tau's
//! data directory ([`crate::BUILTIN_DIR`]) as the host starts, so the
//! model reads their reference files from there like any skill's.

use std::{collections::BTreeSet, fs, io, path::Path};

/// Each shipped skill: its name, and its files by their paths in its
/// folder.
pub const SKILLS: &[(&str, &[(&str, &str)])] =
    include!(concat!(env!("OUT_DIR"), "/shipped.rs"));

/// The file in [`crate::BUILTIN_DIR`] naming the skills the last
/// install wrote, so a skill tau stopped shipping is removed.
pub const MANIFEST: &str = ".tau-skills";

/// Writes every shipped skill into `dir`, replacing what each one's
/// folder held, and removes those an earlier install wrote that are no
/// longer shipped. Other plugins' skills in `dir` are left alone. It
/// blocks: call it from `spawn_blocking`.
pub fn install(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let manifest = dir.join(MANIFEST);
    let shipped: BTreeSet<&str> =
        SKILLS.iter().map(|(name, _)| *name).collect();
    let before = fs::read_to_string(&manifest).unwrap_or_default();
    for gone in before.lines().filter(|name| !shipped.contains(name)) {
        let folder = dir.join(gone);
        // A name from the manifest is a folder of ours, never a path.
        if !gone.is_empty()
            && !gone.contains(['/', '\\', '.'])
            && folder.exists()
        {
            fs::remove_dir_all(folder)?;
        }
    }
    for (name, files) in SKILLS {
        let root = dir.join(name);
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        for (path, text) in *files {
            let file = root.join(path);
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(file, text)?;
        }
    }
    let names: Vec<&str> = shipped.into_iter().collect();
    let partial = dir.join(format!("{MANIFEST}.partial"));
    fs::write(&partial, names.join("\n") + "\n")?;
    fs::rename(partial, manifest)
}

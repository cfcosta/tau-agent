//! Embeds the skills tau ships, from `skills/`: one folder each, every
//! file in it. The list is read from the folder, so adding a skill is
//! adding its folder.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

fn main() {
    let root =
        Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("skills");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut code = String::from("&[\n");
    for skill in sorted(&root).into_iter().filter(|path| path.is_dir()) {
        let name = skill.file_name().unwrap().to_str().unwrap().to_owned();
        let mut files = Vec::new();
        walk(&skill, &mut files);
        writeln!(code, "    ({name:?}, &[").unwrap();
        for file in files {
            let path = file.strip_prefix(&skill).unwrap().to_str().unwrap();
            let path = path.replace(std::path::MAIN_SEPARATOR, "/");
            writeln!(
                code,
                "        ({path:?}, include_str!({:?})),",
                file.to_str().unwrap()
            )
            .unwrap();
        }
        code.push_str("    ]),\n");
    }
    code.push(']');
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    fs::write(out.join("shipped.rs"), code).unwrap();
}

/// `dir`'s entries, by path.
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

/// Every file under `dir`, by path.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    for path in sorted(dir) {
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

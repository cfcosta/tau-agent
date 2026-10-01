//! Embeds the brand marks in `assets/brand/` when they are there.
//!
//! OpenAI's ChatGPT mark and GitHub's mark are their owners' artwork: tau
//! does not ship or redraw them. Dropping the approved files in as
//! `assets/brand/chatgpt-mark.svg` and `assets/brand/github-mark.svg`
//! puts them on the onboarding buttons and tiles; without them, a neutral
//! placeholder stands in.

use std::{env, fs, path::Path};

const MARKS: [(&str, &str); 2] = [
    ("CHATGPT_MARK", "chatgpt-mark.svg"),
    ("GITHUB_MARK", "github-mark.svg"),
];

fn main() {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/brand");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut out = String::new();
    for (name, file) in MARKS {
        let path = dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let value = if path.is_file() {
            format!("Some(include_bytes!({:?}))", path.display().to_string())
        } else {
            "None".to_owned()
        };
        out.push_str(&format!("pub const {name}: Option<&[u8]> = {value};\n"));
    }
    let target = Path::new(&env::var("OUT_DIR").unwrap()).join("brand.rs");
    fs::write(target, out).unwrap();
}

//! The interface never names the memory plugin but to list it (ADR
//! 0006, ADR 0044): what memory adds, it adds through the points.

use std::path::Path;

/// Every Rust file under `dir`, with its text.
fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((path.display().to_string(), text));
        }
    }
}

#[test]
fn only_the_plugin_list_names_memory() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 10, "the sources are read");
    let naming: Vec<&str> = files
        .iter()
        .filter(|(_, text)| text.contains("tau_memory"))
        .map(|(path, _)| path.as_str())
        .collect();
    let list = src.join("plugins.rs").display().to_string();
    assert_eq!(naming, [list.as_str()]);
}

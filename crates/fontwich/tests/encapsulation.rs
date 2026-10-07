//! Checks of the source's layout that the compiler does not make.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// A module's children are private: its `mod.rs` re-exports what other
/// modules use.
#[test]
fn child_modules_are_private() {
    // Spelled in two halves, so that this file does not name them.
    let widened = [
        concat!("pub(crate) ", "mod "),
        concat!("pub(super) ", "mod "),
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    files.sort();

    let mut report = String::new();
    for path in &files {
        let file = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(path) else {
            let _ = writeln!(report, "{file}: unreadable");
            continue;
        };
        for (at, line) in text.lines().enumerate() {
            if widened
                .iter()
                .any(|start| line.trim_start().starts_with(start))
            {
                let _ = writeln!(report, "{file}:{}: {}", at + 1, line.trim());
            }
        }
    }
    assert!(
        report.is_empty(),
        "\nThese modules are wider than private; make them `mod`, and \
         re-export from the parent's `mod.rs` what other modules use:\n\n{report}"
    );
}

/// Every `.rs` file under `dir`.
fn collect(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}

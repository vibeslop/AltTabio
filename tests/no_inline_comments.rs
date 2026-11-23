use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn find_inline_comments(path: &Path) -> io::Result<Vec<(usize, String)>> {
    let content = fs::read_to_string(path)?;
    let mut violations = Vec::new();
    let mut in_safety_block = false;

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            in_safety_block = false;
            continue;
        }
        if trimmed.starts_with("//") {
            if trimmed.starts_with("// SAFETY:") {
                in_safety_block = true;
            } else if !in_safety_block {
                violations.push((index + 1, trimmed.to_owned()));
            }
        } else {
            in_safety_block = false;
        }
    }

    Ok(violations)
}

#[test]
fn src_has_no_non_safety_inline_comments() -> io::Result<()> {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src_dir, &mut files)?;
    files.sort();

    let mut violations = Vec::new();
    for file in files {
        for (line_number, text) in find_inline_comments(&file)? {
            let relative = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&file);
            violations.push(format!("{}:{line_number}: {text}", relative.display()));
        }
    }

    assert!(
        violations.is_empty(),
        "Inline comments are banned in src/. Keep // SAFETY: invariants and /// doc comments; \
express everything else through types, names, lints, or tests.\nFound {} violation(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
    Ok(())
}

#![allow(clippy::absurd_extreme_comparisons)]

use std::fs;
use std::path::{Path, PathBuf};

const MAX_UNWRAP: usize = 14;
const MAX_EXPECT: usize = 0;
const MAX_PANIC: usize = 0;
const MAX_UNREACHABLE: usize = 0;
const MAX_TODO: usize = 0;
const MAX_UNIMPLEMENTED: usize = 0;
// Initialized to the current production count: these ratchet only downward.
// Inline `#[cfg(test)] mod tests { ... }` bodies in production are debt to
// extract to `*_test.rs` companion files in future housekeeping passes.
const MAX_INLINE_TEST_MODULES: usize = 17;
const MAX_TEST_ATTR_IN_PRODUCTION: usize = 17;

struct SourceFile {
    path: PathBuf,
    /// Raw file content, for structural inline-test detection.
    raw: String,
    /// Content with the trailing `#[cfg(test)]` section stripped, for
    /// banned-pattern counting on production code only.
    content: String,
}

fn is_test_companion(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    stem.ends_with("_test")
        || stem.ends_with("_tests")
        || name.ends_with(".test.rs")
}

fn source_files() -> Vec<SourceFile> {
    let mut files = Vec::new();
    collect_rs_files(Path::new("src"), &mut files);
    files
}

fn collect_rs_files(dir: &Path, out: &mut Vec<SourceFile>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
            continue;
        }

        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }

        if is_test_companion(&path) {
            continue;
        }

        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };

        let mut content = raw.clone();
        if let Some((production, _)) = content.split_once("#[cfg(test)]") {
            content = production.to_string();
        }

        out.push(SourceFile {
            path,
            raw,
            content,
        });
    }
}

fn count_in_source(files: &[SourceFile], pattern: &str) -> usize {
    files
        .iter()
        .map(|file| {
            file.content
                .lines()
                .filter(|line| line.contains(pattern))
                .count()
        })
        .sum()
}

fn assert_budget(files: &[SourceFile], pattern: &str, max: usize, label: &str) {
    let count = count_in_source(files, pattern);
    assert!(
        count <= max,
        "{label} budget exceeded: found {count}, max {max}."
    );
}

/// Structural inline-test detection runs inline in the two ratchet tests
/// below against **raw** file content (not the stripped production slice), so
/// a `#[cfg(test)] mod tests { ... }` block in a production file is always
/// flagged.

#[test]
fn unwrap_budget() {
    let files = source_files();
    assert_budget(&files, ".unwrap()", MAX_UNWRAP, ".unwrap()");
}

#[test]
fn expect_budget() {
    let files = source_files();
    assert_budget(&files, ".expect(", MAX_EXPECT, ".expect(");
}

#[test]
fn panic_budget() {
    let files = source_files();
    assert_budget(&files, "panic!(", MAX_PANIC, "panic!(");
}

#[test]
fn unreachable_budget() {
    let files = source_files();
    assert_budget(&files, "unreachable!(", MAX_UNREACHABLE, "unreachable!(");
}

#[test]
fn todo_budget() {
    let files = source_files();
    assert_budget(&files, "todo!(", MAX_TODO, "todo!(");
}

#[test]
fn unimplemented_budget() {
    let files = source_files();
    assert_budget(
        &files,
        "unimplemented!(",
        MAX_UNIMPLEMENTED,
        "unimplemented!(",
    );
}

/// Returns the subset of inline-test-module violations (structural boundary
/// break), separate from bare `#[test]`-attr count. Both share the raw scan but
/// have independent ratchets so they can be lowered independently.
#[test]
fn no_inline_test_modules() {
    let files = source_files();
    let violations: Vec<String> = files
        .iter()
        .filter(|file| {
            file.raw.contains("#[cfg(test)]") && file.raw.contains("mod tests {")
        })
        .map(|file| format!("{}: inline #[cfg(test)] mod tests {{", file.path.display()))
        .collect();
    assert!(
        violations.len() <= MAX_INLINE_TEST_MODULES,
        "inline test module budget exceeded: found {}, max {}. Violations:\n{}",
        violations.len(),
        MAX_INLINE_TEST_MODULES,
        violations.join("\n")
    );
}

#[test]
fn no_test_attr_in_production() {
    let files = source_files();
    let violations: Vec<String> = files
        .iter()
        .filter(|file| {
            file.raw.contains("#[test]") || file.raw.contains("#[tokio::test]")
        })
        .map(|file| format!("{}: #[test] in production file", file.path.display()))
        .collect();
    assert!(
        violations.len() <= MAX_TEST_ATTR_IN_PRODUCTION,
        "#[test] in production budget exceeded: found {}, max {}. Violations:\n{}",
        violations.len(),
        MAX_TEST_ATTR_IN_PRODUCTION,
        violations.join("\n")
    );
}

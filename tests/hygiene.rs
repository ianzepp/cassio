#![allow(clippy::absurd_extreme_comparisons)]

use std::fs;
use std::path::Path;

const MAX_UNWRAP: usize = 14;
const MAX_EXPECT: usize = 0;
const MAX_PANIC: usize = 0;
const MAX_UNREACHABLE: usize = 0;
const MAX_TODO: usize = 0;
const MAX_UNIMPLEMENTED: usize = 0;

struct SourceFile {
    content: String,
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

        let Ok(mut content) = fs::read_to_string(&path) else {
            continue;
        };

        if let Some((production, _)) = content.split_once("#[cfg(test)]") {
            content = production.to_string();
        }

        out.push(SourceFile { content });
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

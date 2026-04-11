use std::path::Path;
use std::process::Command;

use crate::config::GitConfig;
use crate::error::CassioError;

pub fn sync_before_writing(dir: &Path, git: &GitConfig) -> Result<(), CassioError> {
    if !git.commit || !is_git_repo(dir) {
        return Ok(());
    }

    if !worktree_is_clean(dir)? {
        eprintln!("git: skipping pull --ff-only because worktree is not clean");
        return Ok(());
    }

    let pull = Command::new("git")
        .args(["pull", "--ff-only"])
        .current_dir(dir)
        .output()
        .map_err(|e| CassioError::Other(format!("git pull failed: {e}")))?;

    if pull.status.success() {
        eprintln!("git: synced");
    } else {
        let stderr = String::from_utf8_lossy(&pull.stderr);
        if !stderr.trim().is_empty() {
            eprintln!("git pull --ff-only failed: {stderr}");
        }
    }

    Ok(())
}

/// If git.commit is enabled, stage all changes in `dir` and commit.
/// If git.push is also enabled, push after committing.
/// Silently skips if `dir` is not a git repo.
pub fn auto_commit_and_push(dir: &Path, message: &str, git: &GitConfig) -> Result<(), CassioError> {
    if !git.commit {
        return Ok(());
    }

    if !is_git_repo(dir) {
        return Ok(());
    }

    // Stage all changes
    let add = Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir)
        .output()
        .map_err(|e| CassioError::Other(format!("git add failed: {e}")))?;

    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr);
        eprintln!("git add failed: {stderr}");
        return Ok(());
    }

    // Check if there's anything to commit
    let diff = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(dir)
        .status()
        .map_err(|e| CassioError::Other(format!("git diff failed: {e}")))?;

    if diff.success() {
        // Nothing staged, skip
        return Ok(());
    }

    // Commit
    let commit = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(dir)
        .output()
        .map_err(|e| CassioError::Other(format!("git commit failed: {e}")))?;

    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        eprintln!("git commit failed: {stderr}");
        return Ok(());
    }

    eprintln!("git: committed");

    // Push if configured
    if git.push {
        let push = Command::new("git")
            .args(["push"])
            .current_dir(dir)
            .output()
            .map_err(|e| CassioError::Other(format!("git push failed: {e}")))?;

        if push.status.success() {
            eprintln!("git: pushed");
        } else {
            let stderr = String::from_utf8_lossy(&push.stderr);
            eprintln!("git push failed: {stderr}");
        }
    }

    Ok(())
}

fn is_git_repo(dir: &Path) -> bool {
    let status = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    matches!(status, Ok(s) if s.success())
}

fn worktree_is_clean(dir: &Path) -> Result<bool, CassioError> {
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .map_err(|e| CassioError::Other(format!("git status failed: {e}")))?;

    if !status.status.success() {
        return Ok(false);
    }

    Ok(status.stdout.is_empty())
}

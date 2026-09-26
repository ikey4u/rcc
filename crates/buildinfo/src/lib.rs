//! Build-script helper that stamps workspace binaries with their git version.

use std::{path::Path, process::Command};

/// Sets `env_name` for the crate being built to `{pkg}-{shortsha}`, plus
/// `+dirty` when a tracked input differs from HEAD.
///
/// `inputs` are git pathspecs relative to the crate directory (the build
/// script's working directory) naming everything the binary is built from.
/// Untracked files never mark a build dirty, matching `git describe --dirty`.
pub fn emit_git_version(env_name: &str, inputs: &[&str]) {
    let pkg_version =
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut version = pkg_version.clone();
    // A crate extracted into some other repository must not report that
    // repository's commit.
    if git_output(&["ls-files", "--", "Cargo.toml"]).is_some() {
        watch_git_metadata();
        watch_tracked_inputs(inputs);
        if let Some(commit) = git_output(&["log", "-1", "--format=%h"]) {
            version = format!("{pkg_version}-{commit}");
            if git_dirty(inputs) {
                version.push_str("+dirty");
            }
        }
    }
    println!("cargo:rustc-env={env_name}={version}");
}

fn watch_git_metadata() {
    // logs/HEAD is appended on every commit, checkout, and reset, including
    // commits to a packed ref whose loose file does not exist yet.
    for spec in ["HEAD", "index", "logs/HEAD", "packed-refs"] {
        watch_git_path(spec);
    }
    if let Some(refname) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&refname);
    }
}

fn watch_git_path(spec: &str) {
    let Some(path) = git_output(&["rev-parse", "--git-path", spec]) else {
        return;
    };
    if Path::new(&path).exists() {
        println!("cargo:rerun-if-changed={path}");
    }
}

// Unstaged edits change neither HEAD nor the index, so the inputs themselves
// must be watched for a clean build to notice it became dirty.
fn watch_tracked_inputs(inputs: &[&str]) {
    let mut args = vec!["ls-files", "--"];
    args.extend_from_slice(inputs);
    if let Some(files) = git_output(&args) {
        for file in files.lines() {
            println!("cargo:rerun-if-changed={file}");
        }
    }
}

fn git() -> Command {
    let mut command = Command::new("git");
    // Without this, `status` refreshes the index this helper watches and
    // every build would rerun the build script.
    command.arg("--no-optional-locks");
    command
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = git().args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn git_dirty(inputs: &[&str]) -> bool {
    git()
        .args(["status", "--porcelain", "--untracked-files=no", "--"])
        .args(inputs)
        .output()
        .map(|output| output.status.success() && !output.stdout.is_empty())
        .unwrap_or(false)
}

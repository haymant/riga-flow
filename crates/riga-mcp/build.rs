//! Captures the short git SHA at build time so `health` can report it.
//! Degrades to "unknown" when git is unavailable (e.g. a source tarball build).

use std::process::Command;

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn main() {
    let sha =
        git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=RIGA_GIT_SHA={sha}");
    // This crate is usually checked out as a submodule, where `.git` is a gitlink
    // *file* — so the old hard-coded `../../.git/HEAD` never existed, and cargo
    // re-ran this script (relinking the LTO binary) on every single build. Ask
    // git for the real HEAD path instead.
    match git_output(&["rev-parse", "--git-path", "HEAD"]) {
        Some(head) => println!("cargo:rerun-if-changed={head}"),
        None => println!("cargo:rerun-if-changed=build.rs"),
    }
    println!("cargo:rerun-if-env-changed=RIGA_GIT_SHA");
}

//! Build identity of the gateway binary: the git commit it was built from and whether the Rust sources were modified at the time.
//! The full-stack browser suite reads it from `/api/health` and refuses to run against a binary that does not match the source tree
//! (2026-10-26: a locked executable was not rebuilt and a whole suite ran against the old binary without anyone noticing).
use std::process::Command;

fn git(args: &[&str]) -> String {
    Command::new("git").args(args).current_dir("../..").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn main() {
    let commit = git(&["rev-parse", "HEAD"]);
    // only the code that ends up in the binary counts: benchmark results and docs do not make a build "dirty"
    let dirty = !git(&["status", "--porcelain", "--", "crates", "Cargo.toml", "Cargo.lock"]).is_empty();
    println!("cargo:rustc-env=GF_GIT_COMMIT={}", if commit.is_empty() { "unknown" } else { &commit });
    println!("cargo:rustc-env=GF_GIT_DIRTY={dirty}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");
}

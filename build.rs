use std::process::Command;

fn main() {
    // Embed git hash in the binary for dev version identification
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .unwrap_or_default();

    let git_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    let version = env!("CARGO_PKG_VERSION");
    let long_version = if git_hash.trim().is_empty() {
        version.to_string()
    } else if git_dirty {
        format!("{version}+{}.dirty", git_hash.trim())
    } else {
        format!("{version}+{}", git_hash.trim())
    };

    println!("cargo::rustc-env=HILAN_LONG_VERSION={long_version}");

    // Rerun if git HEAD changes
    println!("cargo::rerun-if-changed=.git/HEAD");
    println!("cargo::rerun-if-changed=.git/refs/");
}

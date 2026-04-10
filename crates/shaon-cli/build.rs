use std::process::Command;

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|text| text.trim().to_string())
            } else {
                None
            }
        })
        .filter(|text| !text.is_empty())
}

fn main() {
    let git_hash = git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let git_dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|output| !output.stdout.is_empty())
        .unwrap_or(false);

    let version = env!("CARGO_PKG_VERSION");
    let long_version = if git_hash.is_empty() {
        version.to_string()
    } else if git_dirty {
        format!("{version}+{git_hash}.dirty")
    } else {
        format!("{version}+{git_hash}")
    };

    println!("cargo::rustc-env=SHAON_LONG_VERSION={long_version}");

    if let Some(head_path) = git_output(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo::rerun-if-changed={head_path}");
    }
    if let Some(refs_path) = git_output(&["rev-parse", "--git-path", "refs"]) {
        println!("cargo::rerun-if-changed={refs_path}");
    }
}

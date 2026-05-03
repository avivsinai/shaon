use std::fs;
use std::path::Path;

const MACOS_IDENTIFIER: &str = "io.github.avivsinai.shaon";
const OLD_MACOS_IDENTIFIER: &str = concat!("com.", "avivsinai.shaon");

fn read_repo_file(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap_or_else(|err| panic!("read {path}: {err}"))
}

#[test]
fn macos_codesign_uses_stable_identifier_requirement_everywhere() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let helper = read_repo_file(root, "scripts/codesign-macos.sh");
    let run_sh = read_repo_file(root, "scripts/run.sh");
    let release_workflow = read_repo_file(root, ".github/workflows/release.yml");

    assert!(helper.contains("--identifier \"$identifier\""));
    assert!(helper.contains("-r=\"designated => identifier \\\"$identifier\\\"\""));
    assert!(run_sh.contains(&format!("MACOS_CODESIGN_ID=\"{MACOS_IDENTIFIER}\"")));
    assert!(run_sh.contains("codesign-macos.sh"));
    assert!(release_workflow.contains(&format!(
        "./scripts/codesign-macos.sh target/${{{{ matrix.target }}}}/release/${{{{ matrix.artifact }}}} {MACOS_IDENTIFIER} darwin"
    )));

    for (path, content) in [
        ("scripts/codesign-macos.sh", helper),
        ("scripts/run.sh", run_sh),
        (".github/workflows/release.yml", release_workflow),
        (
            "scripts/setup-codesign.sh",
            read_repo_file(root, "scripts/setup-codesign.sh"),
        ),
    ] {
        assert!(
            !content.contains(OLD_MACOS_IDENTIFIER),
            "{path} still references the old macOS codesign identifier"
        );
    }
}

//! Build information: the git commit, through vergen-gitcl. Without git (a source archive,
//! a container build) the SHA is a placeholder that the binary shows as `unknown`; the
//! `VERGEN_GIT_SHA` environment variable overrides it.

use std::error::Error;
use std::path::Path;
use std::process::Command;

use vergen_gitcl::{Emitter, GitclBuilder};

fn main() -> Result<(), Box<dyn Error>> {
    let git = GitclBuilder::default().sha(false).build()?;
    // Quiet: without git the placeholder is expected and needs no warning.
    Emitter::default().quiet().add_instructions(&git)?.emit()?;
    println!("cargo:rerun-if-env-changed=VERGEN_GIT_SHA");
    watch_git_refs();
    Ok(())
}

/// Re-runs the build script when HEAD moves, including after `git pack-refs` and in
/// linked worktrees, which vergen alone does not notice.
fn watch_git_refs() {
    let Ok(out) = Command::new("git")
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ])
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
        return;
    };
    let paths = [
        format!("{git_dir}/HEAD"),
        format!("{common_dir}/refs/heads"),
        format!("{common_dir}/packed-refs"),
    ];
    for path in paths.iter().filter(|path| Path::new(path).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }
}

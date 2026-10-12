//! Stamp the build with the git commit, for the About dialog. Falls back to
//! "unknown" when built outside a git checkout (e.g. from a source tarball).
//! On Windows, also embed the app icon (resources/sheaf.rc).

use std::process::Command;

fn main() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let hash = git(&["rev-parse", "--short=10", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let commit = if dirty { format!("{hash}-dirty") } else { hash };
    println!("cargo:rustc-env=SHEAF_GIT_COMMIT={commit}");

    // Host-gated like the build-dependency: cross builds to Windows go without.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=resources/sheaf.rc");
        println!("cargo:rerun-if-changed=resources/sheaf.ico");
        embed_resource::compile("resources/sheaf.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the app icon");
    }

    // Re-run when HEAD moves or the index changes (commit, checkout, add).
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/index");
        if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={dir}/{head_ref}");
        }
    }
}

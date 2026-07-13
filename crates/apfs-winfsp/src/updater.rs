//! Self-update from GitHub Releases.
//!
//! The CI workflow publishes `apfs-windows-x64-vX.Y.Z.zip` on every version
//! tag. This module compares the running binary's version against the
//! latest release and, when newer, downloads the zip and swaps the
//! binaries in place (Windows allows renaming a running exe, so the swap
//! is safe; the new version takes effect on next start).
//!
//! The repository is taken from Cargo.toml's `repository` field at compile
//! time — keep it pointing at the real GitHub URL.

use self_update::backends::github::Update;
use self_update::cargo_crate_version;

/// Parse "https://github.com/<owner>/<repo>" from CARGO_PKG_REPOSITORY.
fn repo() -> Result<(String, String), String> {
    let url = env!("CARGO_PKG_REPOSITORY");
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| format!("repository URL not a github.com URL: {url}"))?;
    let mut it = rest.trim_end_matches('/').trim_end_matches(".git").split('/');
    match (it.next(), it.next()) {
        (Some(o), Some(r)) if !o.is_empty() && !r.is_empty() && o != "your-org" => {
            Ok((o.to_string(), r.to_string()))
        }
        _ => Err(format!(
            "set the real GitHub URL in the workspace Cargo.toml `repository` field (currently {url})"
        )),
    }
}

/// Check for a newer release and install it. Returns Ok(true) if an update
/// was installed (effective on next start), Ok(false) if already current.
pub fn run_update(quiet: bool) -> Result<bool, String> {
    let (owner, name) = repo()?;
    let mut updated = false;

    // Private repos return 404 to anonymous API calls; a token in the
    // GITHUB_TOKEN environment variable authenticates the check.
    let token = std::env::var("GITHUB_TOKEN").ok();

    // Update both binaries shipped in the release zip.
    //
    // IMPORTANT: self_update installs to the *running executable's* path
    // by default, so without an explicit install path the second loop
    // iteration would overwrite apfs-mount.exe with apfs.exe. Each binary
    // must be routed to its own name in the exe's directory.
    let exe_dir = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?
        .parent()
        .ok_or("exe has no parent directory")?
        .to_path_buf();

    for bin in ["apfs-mount.exe", "apfs.exe"] {
        let install_path = exe_dir.join(bin);
        let mut cfg = Update::configure();
        cfg.repo_owner(&owner)
            .repo_name(&name)
            // Assets are named apfs-windows-x64-vX.Y.Z.zip; `target` is
            // matched as a substring against asset names.
            .target("windows-x64")
            .bin_name(bin)
            .bin_path_in_archive(bin)
            .bin_install_path(&install_path)
            .current_version(cargo_crate_version!())
            .no_confirm(true)
            .show_download_progress(!quiet)
            .show_output(!quiet);
        if let Some(t) = &token {
            cfg.auth_token(t);
        }
        let status = cfg
            .build()
            .map_err(|e| format!("updater config: {e}"))?
            .update()
            .map_err(|e| format!("update {bin}: {e}"))?;
        if status.updated() {
            updated = true;
            if !quiet {
                println!("{bin} -> {}", status.version());
            }
        }
    }
    Ok(updated)
}

/// Background updater for watch mode: check shortly after start, then
/// daily. Updates are applied silently and take effect when the process
/// restarts (next logon for the auto-mount task).
pub fn spawn_background_checker() {
    std::thread::spawn(|| {
        // Give the machine a minute to settle after logon.
        std::thread::sleep(std::time::Duration::from_secs(60));
        loop {
            match run_update(true) {
                Ok(true) => {
                    println!("update installed; it takes effect on the next restart of apfs-mount")
                }
                Ok(false) => {}
                Err(e) => eprintln!("update check failed: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
        }
    });
}

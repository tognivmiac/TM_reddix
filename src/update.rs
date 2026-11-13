//! Update check for this personal fork.
//!
//! "Latest" means the version declared by the local source checkout this
//! binary was built from, not an upstream GitHub release. When the checkout's
//! `Cargo.toml` is ahead of the running binary, the updater rebuilds from that
//! checkout and replaces the running executable. Nothing is fetched from the
//! network.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use semver::Version;

pub const SKIP_UPDATE_ENV: &str = "REDDIX_SKIP_UPDATE_CHECK";
/// Overrides the source checkout used for update checks and rebuilds.
pub const SOURCE_DIR_ENV: &str = "REDDIX_SOURCE_DIR";

/// The checkout this binary was compiled from.
const BUILD_SOURCE_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FORCE_VERSION_ENV: &str = "REDDIX_FORCE_UPDATE_VERSION";
const FORCE_URL_ENV: &str = "REDDIX_FORCE_UPDATE_URL";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: Version,
    /// Where the newer version lives: the local source checkout.
    pub release_url: String,
    pub tag: String,
}

/// Source checkout that defines the latest version of this fork.
pub fn source_dir() -> PathBuf {
    env::var_os(SOURCE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(BUILD_SOURCE_DIR))
}

pub fn check_for_update(current: &Version) -> Result<Option<UpdateInfo>> {
    if let Some(update) = forced_update(current)? {
        return Ok(Some(update));
    }

    let dir = source_dir();
    let manifest = dir.join("Cargo.toml");
    if !manifest.is_file() {
        // The checkout was moved or removed; there is nothing to compare with.
        return Ok(None);
    }

    let text =
        fs::read_to_string(&manifest).with_context(|| format!("read {}", manifest.display()))?;
    let version = manifest_version(&text)
        .with_context(|| format!("read package version from {}", manifest.display()))?;

    if &version > current {
        Ok(Some(UpdateInfo {
            tag: format!("v{version}"),
            version,
            release_url: dir.display().to_string(),
        }))
    } else {
        Ok(None)
    }
}

/// Extracts `version` from the `[package]` table of a Cargo manifest.
fn manifest_version(manifest: &str) -> Result<Version> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "version" {
            continue;
        }
        let value = value.split('#').next().unwrap_or_default().trim();
        let value = value.trim_matches(|ch| ch == '"' || ch == '\'');
        return Version::parse(value)
            .with_context(|| format!("parse {value:?} as semantic version"));
    }
    Err(anyhow!("no [package] version found"))
}

fn forced_update(current: &Version) -> Result<Option<UpdateInfo>> {
    let forced_version = match env::var(FORCE_VERSION_ENV) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => {
            bail!("read {FORCE_VERSION_ENV}: {err}");
        }
    };

    let trimmed = forced_version.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let version = Version::parse(trimmed)
        .with_context(|| format!("parse {FORCE_VERSION_ENV}={trimmed:?} as semantic version"))?;

    if &version <= current {
        return Ok(None);
    }

    let release_url = env::var(FORCE_URL_ENV)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| source_dir().display().to_string());
    let tag = format!("v{version}");

    Ok(Some(UpdateInfo {
        version,
        release_url,
        tag,
    }))
}

/// Rebuilds from the local source checkout and replaces the running binary.
pub fn install_update(_info: &UpdateInfo) -> Result<()> {
    let dir = source_dir();
    let output = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&dir)
        .output()
        .with_context(|| format!("run cargo build --release in {}", dir.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!(
            "cargo build exited with status {}: {}",
            output.status,
            tail.join(" | ")
        );
    }

    let built = dir.join("target").join("release").join(format!(
        "{}{}",
        crate::BIN_NAME,
        env::consts::EXE_SUFFIX
    ));
    if !built.is_file() {
        bail!("built binary not found at {}", built.display());
    }

    let current = env::current_exe().context("locate the running executable")?;
    // Resolve symlinks (e.g. a Homebrew bin link) so the real file is replaced.
    let target = fs::canonicalize(&current).unwrap_or(current);
    if fs::canonicalize(&built).ok().as_deref() == Some(target.as_path()) {
        // Running straight out of target/release: the build already updated it.
        return Ok(());
    }
    replace_executable(&built, &target)
}

/// Copies `built` next to `target` and renames it into place, so the running
/// binary is swapped atomically and never left half-written.
fn replace_executable(built: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", target.display()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".reddix-update-")
        .tempfile_in(parent)
        .with_context(|| format!("create staging file in {}", parent.display()))?;
    let mut source = fs::File::open(built).with_context(|| format!("open {}", built.display()))?;
    std::io::copy(&mut source, staged.as_file_mut())
        .with_context(|| format!("copy {} into place", built.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))
            .context("mark the new binary as executable")?;
    }

    staged
        .persist(target)
        .with_context(|| format!("replace {}", target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn guard_env() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn forced_update_returns_info_when_version_is_newer() {
        let _guard = guard_env();
        env::set_var(FORCE_VERSION_ENV, "0.2.1");
        env::set_var(FORCE_URL_ENV, "https://example.com/releases/0.2.1");
        let current = Version::parse("0.1.0").unwrap();
        let info = forced_update(&current).unwrap().unwrap();
        assert_eq!(info.version, Version::parse("0.2.1").unwrap());
        assert_eq!(info.release_url, "https://example.com/releases/0.2.1");
        assert_eq!(info.tag, "v0.2.1");
        env::remove_var(FORCE_VERSION_ENV);
        env::remove_var(FORCE_URL_ENV);
    }

    #[test]
    fn forced_update_is_ignored_when_version_not_newer() {
        let _guard = guard_env();
        env::set_var(FORCE_VERSION_ENV, "0.1.0");
        let current = Version::parse("0.1.0").unwrap();
        assert!(forced_update(&current).unwrap().is_none());
        env::remove_var(FORCE_VERSION_ENV);
    }

    #[test]
    fn manifest_version_reads_package_table_only() {
        let manifest = r#"
[workspace.package]
version = "9.9.9"

[package]
name = "reddix"
version = "0.2.9+tm.2" # fork build

[dependencies]
version = "1.0.0"
"#;
        assert_eq!(
            manifest_version(manifest).unwrap(),
            Version::parse("0.2.9+tm.2").unwrap()
        );
        assert!(manifest_version("[dependencies]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn fork_suffix_orders_builds() {
        let parse = |raw: &str| Version::parse(raw).unwrap();
        assert!(parse("0.2.9+tm.2") > parse("0.2.9+tm.1"));
        assert!(parse("0.2.9+tm.10") > parse("0.2.9+tm.9"));
        assert!(parse("0.2.9+tm.1") > parse("0.2.9"));
        assert!(parse("0.2.10") > parse("0.2.9+tm.5"));
    }

    #[test]
    fn check_compares_against_source_checkout() {
        let _guard = guard_env();
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"reddix\"\nversion = \"0.2.9+tm.2\"\n",
        )
        .unwrap();
        env::set_var(SOURCE_DIR_ENV, dir.path());

        let older = Version::parse("0.2.9+tm.1").unwrap();
        let info = check_for_update(&older).unwrap().unwrap();
        assert_eq!(info.version, Version::parse("0.2.9+tm.2").unwrap());
        assert_eq!(info.release_url, dir.path().display().to_string());

        let same = Version::parse("0.2.9+tm.2").unwrap();
        assert!(check_for_update(&same).unwrap().is_none());

        // A missing checkout is not an error; there is just nothing newer.
        env::set_var(SOURCE_DIR_ENV, dir.path().join("missing"));
        assert!(check_for_update(&older).unwrap().is_none());
        env::remove_var(SOURCE_DIR_ENV);
    }
}

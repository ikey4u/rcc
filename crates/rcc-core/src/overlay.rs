//! Local overlay store.
//!
//! An overlay is an explicit, immutable set of target headers, libraries, and
//! frameworks that a user registers on top of a base target profile (ARCH
//! §3.1, §9.4). Like an external SDK, the overlay tree is a local-trust input:
//! it is never copied into the RCC cache or payload. The store only records the
//! overlay's [`OverlayManifest`] and the absolute path to its tree, and binds
//! the two with a content digest so a later view build can detect tampering.

use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::{digest, schema::OverlayManifest, Profile};

/// A registered overlay: its canonical manifest plus the absolute path to the
/// materialized overlay tree on the local machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayRegistration {
    pub manifest: OverlayManifest,
    pub root: PathBuf,
}

/// Directory holding overlay registrations under the RCC home directory.
pub fn overlays_dir(home: &Path) -> PathBuf {
    home.join("overlays")
}

/// Path of a single overlay registration file, namespaced by base profile.
pub fn registration_path(
    home: &Path,
    profile_id: &str,
    overlay_id: &str,
) -> PathBuf {
    overlays_dir(home)
        .join(profile_id)
        .join(format!("{overlay_id}.json"))
}

/// Compute a canonical content digest over an overlay tree: the SHA-256 of a
/// deterministic serialization of every regular file's normalized relative
/// path and content digest, sorted by path. Symlinks and other non-regular
/// files are rejected so the digest cannot be forged through a link that
/// escapes the tree.
pub fn tree_digest(root: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(root).with_context(|| {
        format!("failed to stat overlay root {}", root.display())
    })?;
    ensure!(
        metadata.is_dir(),
        "overlay root is not a directory: {}",
        root.display()
    );
    let mut entries: Vec<(String, String)> = Vec::new();
    for item in WalkDir::new(root).follow_links(false) {
        let item = item.with_context(|| {
            format!("failed to walk overlay root {}", root.display())
        })?;
        let file_type = item.file_type();
        if file_type.is_dir() {
            continue;
        }
        ensure!(
            file_type.is_file(),
            "overlay tree contains a non-regular file: {}",
            item.path().display()
        );
        let relative = item.path().strip_prefix(root).with_context(|| {
            format!("overlay entry {} escaped the root", item.path().display())
        })?;
        let normalized = normalize_relative(relative)?;
        let file_digest = digest::file_sha256(item.path())?;
        entries.push((normalized, file_digest));
    }
    entries.sort();
    let mut hasher = Sha256::new();
    for (path, file_digest) in &entries {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(file_digest.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Register an overlay for its base profile. Validates the manifest, checks it
/// does not promote the base profile's ABI baseline, verifies the declared
/// `tree_digest` matches the on-disk tree, and persists the registration.
pub fn register(
    home: &Path,
    manifest: &OverlayManifest,
    root: &Path,
    base: &Profile,
) -> Result<OverlayRegistration> {
    manifest
        .validate()
        .map_err(|error| anyhow!("invalid overlay manifest: {error}"))?;
    manifest.verify_compatible(base).map_err(|error| {
        anyhow!(
            "overlay is incompatible with profile {}: {error}",
            base.profile_id
        )
    })?;
    ensure!(
        root.is_absolute(),
        "overlay root must be an absolute path: {}",
        root.display()
    );
    let actual = tree_digest(root)?;
    ensure!(
        actual == manifest.tree_digest,
        "overlay tree digest mismatch for {}: manifest declares {}, tree hashes to {}",
        root.display(),
        manifest.tree_digest,
        actual
    );
    let registration = OverlayRegistration {
        manifest: manifest.clone(),
        root: root.to_path_buf(),
    };
    let path =
        registration_path(home, &manifest.profile_id, &manifest.overlay_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("failed to create overlay directory {}", parent.display())
        })?;
    }
    let serialized = serde_json::to_vec_pretty(&registration)
        .context("failed to serialize overlay registration")?;
    fs::write(&path, serialized).with_context(|| {
        format!("failed to write overlay registration {}", path.display())
    })?;
    Ok(registration)
}

/// Load one registered overlay by id. The stored manifest is re-validated and
/// checked against the path it was read from.
pub fn load(
    home: &Path,
    profile_id: &str,
    overlay_id: &str,
) -> Result<OverlayRegistration> {
    let path = registration_path(home, profile_id, overlay_id);
    let bytes = fs::read(&path).with_context(|| {
        format!("failed to read overlay registration {}", path.display())
    })?;
    let registration: OverlayRegistration = serde_json::from_slice(&bytes)
        .with_context(|| {
            format!("failed to parse overlay registration {}", path.display())
        })?;
    registration.manifest.validate().map_err(|error| {
        anyhow!("stored overlay manifest is invalid: {error}")
    })?;
    ensure!(
        registration.manifest.profile_id == profile_id
            && registration.manifest.overlay_id == overlay_id,
        "overlay registration {} does not match its path",
        path.display()
    );
    Ok(registration)
}

/// List every overlay registered for a base profile, ordered by overlay id. A
/// profile with no overlays yields an empty list rather than an error.
pub fn list(home: &Path, profile_id: &str) -> Result<Vec<OverlayRegistration>> {
    let dir = overlays_dir(home).join(profile_id);
    let read_dir = match fs::read_dir(&dir) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Vec::new())
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to list overlays in {}", dir.display())
            })
        }
    };
    let mut stems: Vec<String> = Vec::new();
    for entry in read_dir {
        let entry = entry.with_context(|| {
            format!("failed to read overlay directory {}", dir.display())
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .with_context(|| {
                format!(
                    "overlay registration {} has no readable name",
                    path.display()
                )
            })?;
        stems.push(stem.to_owned());
    }
    stems.sort();
    let mut registrations = Vec::with_capacity(stems.len());
    for stem in &stems {
        registrations.push(load(home, profile_id, stem)?);
    }
    Ok(registrations)
}

fn normalize_relative(path: &Path) -> Result<String> {
    let mut normalized = String::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            bail!("overlay path has an unsafe component: {}", path.display());
        };
        let component = component.to_str().with_context(|| {
            format!("overlay path is not valid UTF-8: {}", path.display())
        })?;
        ensure!(
            !component.is_empty()
                && component != "."
                && component != ".."
                && !component.contains('/')
                && !component.contains('\\')
                && !component.contains(':')
                && !component.contains('\0'),
            "overlay path has an unsafe component: {}",
            path.display()
        );
        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(component);
    }
    ensure!(!normalized.is_empty(), "overlay relative path is empty");
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{registry::find_profile, schema::SCHEMA_VERSION};

    fn base() -> &'static Profile {
        find_profile("linux-aarch64-gnu-glibc217")
            .expect("builtin profile is registered")
    }

    fn write_tree(root: &Path) {
        fs::create_dir_all(root.join("include")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(root.join("include/vendor.h"), b"#pragma once\n").unwrap();
        fs::write(root.join("lib/libvendor.a"), b"!<arch>\n").unwrap();
    }

    fn manifest_for(base: &Profile, tree_digest: String) -> OverlayManifest {
        OverlayManifest {
            schema_version: SCHEMA_VERSION,
            overlay_id: "vendor-openssl".into(),
            profile_id: base.profile_id.clone(),
            revision: 1,
            description: None,
            target_triple: base.target_triple.clone(),
            libc_family: None,
            libc_version: None,
            minimum_os: None,
            include_roots: vec!["include".into()],
            library_roots: vec!["lib".into()],
            framework_roots: Vec::new(),
            tree_digest,
        }
    }

    #[test]
    fn tree_digest_is_deterministic_and_content_bound() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_tree(first.path());
        write_tree(second.path());
        assert_eq!(
            tree_digest(first.path()).unwrap(),
            tree_digest(second.path()).unwrap()
        );

        fs::write(second.path().join("include/vendor.h"), b"changed\n")
            .unwrap();
        assert_ne!(
            tree_digest(first.path()).unwrap(),
            tree_digest(second.path()).unwrap()
        );
    }

    #[test]
    fn register_load_and_list_round_trip() {
        let home = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_tree(tree.path());
        let digest = tree_digest(tree.path()).unwrap();
        let manifest = manifest_for(base(), digest);

        let registered =
            register(home.path(), &manifest, tree.path(), base()).unwrap();
        assert_eq!(registered.manifest, manifest);

        let loaded =
            load(home.path(), &base().profile_id, "vendor-openssl").unwrap();
        assert_eq!(loaded, registered);

        let listed = list(home.path(), &base().profile_id).unwrap();
        assert_eq!(listed, vec![registered]);
    }

    #[test]
    fn register_rejects_a_tree_digest_mismatch() {
        let home = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_tree(tree.path());
        let wrong = "0".repeat(64);
        let manifest = manifest_for(base(), wrong);
        assert!(register(home.path(), &manifest, tree.path(), base()).is_err());
    }

    #[test]
    fn register_rejects_baseline_promotion() {
        let home = tempfile::tempdir().unwrap();
        let tree = tempfile::tempdir().unwrap();
        write_tree(tree.path());
        let digest = tree_digest(tree.path()).unwrap();
        let mut manifest = manifest_for(base(), digest);
        // The base glibc217 profile promises glibc 2.17; requiring 2.28 would
        // raise the baseline.
        manifest.libc_family = Some("glibc".into());
        manifest.libc_version = Some("2.28".into());
        assert!(register(home.path(), &manifest, tree.path(), base()).is_err());
    }

    #[test]
    fn list_is_empty_for_an_unknown_profile() {
        let home = tempfile::tempdir().unwrap();
        assert!(list(home.path(), "linux-x86_64-musl-static")
            .unwrap()
            .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn tree_digest_rejects_symlinks() {
        let tree = tempfile::tempdir().unwrap();
        write_tree(tree.path());
        std::os::unix::fs::symlink("/etc/passwd", tree.path().join("escape"))
            .unwrap();
        assert!(tree_digest(tree.path()).is_err());
    }
}

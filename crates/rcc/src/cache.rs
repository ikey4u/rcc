use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};

pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    let path = match explicit {
        Some(path) => path.to_path_buf(),
        None => dirs::cache_dir()
            .context("could not determine the platform cache directory")?
            .join("rcc"),
    };
    prepare(&path)?;
    Ok(path)
}

fn prepare(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            bail!("refusing symlink cache root {}", path.display());
        }
        if !metadata.is_dir() {
            bail!("cache root {} is not a directory", path.display());
        }
    } else {
        fs::create_dir_all(path).with_context(|| {
            format!("failed to create cache root {}", path.display())
        })?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs::metadata(path).with_context(|| {
            format!("failed to inspect cache root {}", path.display())
        })?;
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(path, permissions).with_context(|| {
                format!(
                    "failed to restrict cache permissions for {}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn prepares_an_explicit_cache() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("cache");
        assert_eq!(resolve(Some(&path)).unwrap(), path);
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_cache_root() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let real = directory.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = directory.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(resolve(Some(&link)).is_err());
    }
}

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Result};

pub const HOME_ENV: &str = "RCC_HOME_DIR";
pub const APPLE_SDK_ROOT_ENV: &str = "RCC_APPLE_SDK_ROOT";
pub const WINDOWS_SDK_ROOT_ENV: &str = "RCC_WINDOWS_SDK_ROOT";
pub const MSVC_TOOLS_ROOT_ENV: &str = "RCC_MSVC_TOOLS_ROOT";

/// Resolve the RCC home directory.
///
/// Override with `--home-dir` or `RCC_HOME_DIR`. Otherwise use the platform
/// local data directory (`dirs::data_local_dir`) plus `rcc`:
///
/// - Linux: `$XDG_DATA_HOME/rcc` or `~/.local/share/rcc`
/// - macOS: `~/Library/Application Support/rcc`
/// - Windows: `%LOCALAPPDATA%\rcc`
///
/// External SDKs live under `$RCC_HOME_DIR/vendor/{macos,windows,msvc,linux}`.
pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        ensure!(!path.as_os_str().is_empty(), "{HOME_ENV} is set but empty");
        return Ok(path.to_path_buf());
    }
    if let Some(value) = env::var_os(HOME_ENV) {
        ensure!(!value.is_empty(), "{HOME_ENV} is set but empty");
        return Ok(PathBuf::from(value));
    }
    Ok(dirs::data_local_dir()
        .context("could not determine the platform data directory")?
        .join("rcc"))
}

pub fn vendor_dir(home: &Path, os: &str) -> PathBuf {
    home.join("vendor").join(os)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_layout_is_under_home() {
        let home = Path::new("/tmp/rcc-home");
        assert_eq!(vendor_dir(home, "macos"), home.join("vendor/macos"));
        assert_eq!(vendor_dir(home, "windows"), home.join("vendor/windows"));
        assert_eq!(vendor_dir(home, "msvc"), home.join("vendor/msvc"));
        assert_eq!(vendor_dir(home, "linux"), home.join("vendor/linux"));
    }
}

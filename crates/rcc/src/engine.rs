#[cfg(rcc_static_llvm)]
use std::ffi::CString;
use std::ffi::{OsStr, OsString};

#[cfg(rcc_static_llvm)]
use anyhow::Context;
use anyhow::{bail, Result};
use rcc_core::ToolKind;

pub const BUILD_ID: &str = env!("RCC_ENGINE_BUILD_ID");

pub fn is_available() -> bool {
    cfg!(rcc_static_llvm)
}

pub fn supports(kind: ToolKind) -> bool {
    matches!(
        kind,
        ToolKind::Cc
            | ToolKind::Cxx
            | ToolKind::Linker
            | ToolKind::Ar
            | ToolKind::Ranlib
    )
}

pub fn run(
    kind: ToolKind,
    argv0: &OsStr,
    arguments: &[OsString],
) -> Result<i32> {
    if !supports(kind) {
        bail!("the statically integrated engine does not provide tool kind {kind}");
    }
    #[cfg(rcc_static_llvm)]
    {
        run_static(kind, argv0, arguments)
    }
    #[cfg(not(rcc_static_llvm))]
    {
        let _ = (argv0, arguments);
        bail!(
            "this development RCC was built without the static LLVM engine; \
             use scripts/build-macos-arm64-release.sh, scripts/build-linux-x86_64-release.sh, or set the pinned LLVM build inputs"
        )
    }
}

#[cfg(rcc_static_llvm)]
fn run_static(
    kind: ToolKind,
    argv0: &OsStr,
    arguments: &[OsString],
) -> Result<i32> {
    let mut storage = Vec::with_capacity(arguments.len() + 1);
    storage.push(
        os_string_to_c_string(argv0).context("invalid multicall argv[0]")?,
    );
    for (index, argument) in arguments.iter().enumerate() {
        storage.push(os_string_to_c_string(argument).with_context(|| {
            format!("tool argument {index} contains an interior NUL byte")
        })?);
    }
    let argc = i32::try_from(storage.len())
        .context("too many native tool arguments")?;
    let mut mutable_argv = storage
        .iter()
        .map(|argument| argument.as_ptr().cast_mut())
        .collect::<Vec<_>>();
    // LLVM follows the C argv convention and may inspect argv[argc].
    mutable_argv.push(std::ptr::null_mut());

    let code = unsafe {
        match kind {
            ToolKind::Cc | ToolKind::Cxx => {
                rcc_clang_main(argc, mutable_argv.as_mut_ptr())
            }
            ToolKind::Linker => rcc_lld_main(
                argc,
                mutable_argv.as_ptr().cast::<*const std::ffi::c_char>(),
            ),
            ToolKind::Ar | ToolKind::Ranlib => {
                rcc_llvm_ar_main(argc, mutable_argv.as_mut_ptr())
            }
            _ => unreachable!("unsupported tool kind was rejected above"),
        }
    };
    Ok(code)
}

#[cfg(all(rcc_static_llvm, unix))]
fn os_string_to_c_string(value: &OsStr) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(CString::new(value.as_bytes())?)
}

#[cfg(all(rcc_static_llvm, windows))]
fn os_string_to_c_string(value: &OsStr) -> Result<CString> {
    let value = value
        .to_str()
        .context("native tool arguments must be valid UTF-8 on Windows")?;
    Ok(CString::new(value)?)
}

#[cfg(all(rcc_static_llvm, not(any(unix, windows))))]
fn os_string_to_c_string(value: &OsStr) -> Result<CString> {
    let value = value.to_str().context(
        "native tool arguments must be valid UTF-8 on this platform",
    )?;
    Ok(CString::new(value)?)
}

#[cfg(rcc_static_llvm)]
extern "C" {
    fn rcc_clang_main(argc: i32, argv: *mut *mut std::ffi::c_char) -> i32;
    fn rcc_lld_main(argc: i32, argv: *const *const std::ffi::c_char) -> i32;
    fn rcc_llvm_ar_main(argc: i32, argv: *mut *mut std::ffi::c_char) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_only_the_first_static_engine_surface() {
        assert!(supports(ToolKind::Cc));
        assert!(supports(ToolKind::Cxx));
        assert!(supports(ToolKind::Linker));
        assert!(supports(ToolKind::Ar));
        assert!(supports(ToolKind::Ranlib));
        assert!(!supports(ToolKind::Objcopy));
    }
}

use crate::schema::{ToolKind, ViewManifest};
use std::collections::BTreeSet;

pub const CROSS_BIN_DIR: &str = "cross-bin";

/// Map a launcher argv0 (`gcc`, `x86_64-unknown-linux-gnu-ranlib`) to a tool.
pub fn tool_kind_from_multicall_name(file_name: &str) -> Option<ToolKind> {
    let lower = file_name.to_ascii_lowercase();
    if let Some(kind) = tool_kind_from_basename(&lower) {
        return Some(kind);
    }
    let mut programs: Vec<&str> = gnu_program_names(ToolKind::Cc)
        .iter()
        .chain(gnu_program_names(ToolKind::Cxx))
        .chain(gnu_program_names(ToolKind::Ar))
        .chain(gnu_program_names(ToolKind::Ranlib))
        .chain(gnu_program_names(ToolKind::Linker))
        .chain(gnu_program_names(ToolKind::Nm))
        .chain(gnu_program_names(ToolKind::Strip))
        .chain(gnu_program_names(ToolKind::Objcopy))
        .chain(gnu_program_names(ToolKind::Dlltool))
        .copied()
        .collect();
    programs.sort_by_key(|name| std::cmp::Reverse(name.len()));
    for program in programs {
        if let Some(prefix) = lower.strip_suffix(&format!("-{program}")) {
            if prefix.contains('-') {
                return tool_kind_from_basename(program);
            }
        }
    }
    None
}

fn tool_kind_from_basename(name: &str) -> Option<ToolKind> {
    match name {
        "cc" | "gcc" | "clang" | "as" | "cpp" => Some(ToolKind::Cc),
        "c++" | "g++" | "clang++" | "cxx" => Some(ToolKind::Cxx),
        "linker" | "ld" | "ld.lld" | "ld64.lld" | "lld-link" | "link" | "lld" => {
            Some(ToolKind::Linker)
        }
        "ar" | "llvm-ar" | "gcc-ar" => Some(ToolKind::Ar),
        "ranlib" | "llvm-ranlib" | "gcc-ranlib" => Some(ToolKind::Ranlib),
        "nm" => Some(ToolKind::Nm),
        "strip" => Some(ToolKind::Strip),
        "objcopy" => Some(ToolKind::Objcopy),
        "dlltool" => Some(ToolKind::Dlltool),
        _ => None,
    }
}

/// GNU/autotools names for a tool. Prefixed copies live in `cross-bin/`.
pub fn gnu_program_names(kind: ToolKind) -> &'static [&'static str] {
    match kind {
        ToolKind::Cc => &["gcc", "cc", "clang", "as", "cpp"],
        ToolKind::Cxx => &["g++", "c++", "clang++"],
        ToolKind::Ar => &["ar", "gcc-ar"],
        ToolKind::Ranlib => &["ranlib", "gcc-ranlib"],
        ToolKind::Linker => &["ld", "ld.lld"],
        ToolKind::Nm => &["nm"],
        ToolKind::Strip => &["strip"],
        ToolKind::Objcopy => &["objcopy"],
        ToolKind::Dlltool => &["dlltool"],
        _ => &[],
    }
}

/// Unprefixed names placed on PATH so `make`/`ranlib`/`ar` hit RCC, not the host.
pub fn unprefixed_path_names(kind: ToolKind) -> &'static [&'static str] {
    match kind {
        ToolKind::Ar => &["ar"],
        ToolKind::Ranlib => &["ranlib"],
        ToolKind::Nm => &["nm"],
        ToolKind::Strip => &["strip"],
        ToolKind::Objcopy => &["objcopy"],
        ToolKind::Dlltool => &["dlltool"],
        _ => &[],
    }
}

pub fn tool_prefixes(target_triple: &str, clang_target: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    push_prefix(&mut prefixes, target_triple);
    push_prefix(&mut prefixes, clang_target);
    if let Some(gnu) = drop_unknown_vendor(target_triple) {
        push_prefix(&mut prefixes, &gnu);
    }
    if let Some(gnu) = drop_unknown_vendor(clang_target) {
        push_prefix(&mut prefixes, &gnu);
    }
    prefixes
}

/// Debian-style prefix when the triple has an `unknown` vendor, else the Rust triple.
pub fn primary_gnu_prefix(target_triple: &str, clang_target: &str) -> String {
    drop_unknown_vendor(target_triple)
        .or_else(|| drop_unknown_vendor(clang_target))
        .unwrap_or_else(|| {
            if target_triple.is_empty() {
                clang_target.to_owned()
            } else {
                target_triple.to_owned()
            }
        })
}

/// Relative paths under the view root (`cross-bin/{triple}-gcc`, `cross-bin/ar`, …).
pub fn relative_paths(view: &ViewManifest) -> Vec<String> {
    relative_paths_for(
        &view.profile.target_triple,
        &view.profile.clang_target,
        &view.profile.tool_kinds,
    )
}

pub fn relative_paths_for(
    target_triple: &str,
    clang_target: &str,
    tool_kinds: &BTreeSet<ToolKind>,
) -> Vec<String> {
    let prefixes = tool_prefixes(target_triple, clang_target);
    let suffix = executable_suffix();
    let mut paths = BTreeSet::new();
    for kind in tool_kinds {
        for program in gnu_program_names(*kind) {
            for prefix in &prefixes {
                paths.insert(format!("{CROSS_BIN_DIR}/{prefix}-{program}{suffix}"));
            }
        }
        for program in unprefixed_path_names(*kind) {
            paths.insert(format!("{CROSS_BIN_DIR}/{program}{suffix}"));
        }
    }
    paths.into_iter().collect()
}

fn push_prefix(prefixes: &mut Vec<String>, value: &str) {
    if value.is_empty() || prefixes.iter().any(|existing| existing == value) {
        return;
    }
    prefixes.push(value.to_owned());
}

fn drop_unknown_vendor(triple: &str) -> Option<String> {
    let mut parts = triple.split('-');
    let arch = parts.next()?;
    let vendor = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    if vendor != "unknown" || rest.is_empty() {
        return None;
    }
    Some(format!("{arch}-{}", rest.join("-")))
}

fn executable_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_gnu_has_autoconf_and_debian_prefixes() {
        let prefixes = tool_prefixes("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu");
        assert!(prefixes.iter().any(|p| p == "x86_64-unknown-linux-gnu"));
        assert!(prefixes.iter().any(|p| p == "x86_64-linux-gnu"));
    }

    #[test]
    fn relative_paths_include_unprefixed_ar_not_gcc() {
        let prefixes = tool_prefixes("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu");
        assert!(!prefixes.is_empty());
        let names = gnu_program_names(ToolKind::Cc);
        assert!(names.contains(&"gcc"));
        let path_names = unprefixed_path_names(ToolKind::Cc);
        assert!(path_names.is_empty());
        assert!(unprefixed_path_names(ToolKind::Ar).contains(&"ar"));
        assert!(unprefixed_path_names(ToolKind::Ranlib).contains(&"ranlib"));
    }

    #[test]
    fn prefixed_gnu_names_map_to_tools() {
        assert_eq!(
            tool_kind_from_multicall_name("x86_64-unknown-linux-gnu-gcc"),
            Some(ToolKind::Cc)
        );
        assert_eq!(
            tool_kind_from_multicall_name("x86_64-linux-gnu-ranlib"),
            Some(ToolKind::Ranlib)
        );
        assert_eq!(
            tool_kind_from_multicall_name("x86_64-unknown-linux-gnu-g++"),
            Some(ToolKind::Cxx)
        );
        assert_eq!(tool_kind_from_multicall_name("gcc"), Some(ToolKind::Cc));
        assert_eq!(tool_kind_from_multicall_name("ar"), Some(ToolKind::Ar));
        assert_eq!(
            tool_kind_from_multicall_name("x86_64-linux-gnu-as"),
            Some(ToolKind::Cc)
        );
    }

    #[test]
    fn relative_paths_cover_autoconf_and_unprefixed_binutils() {
        let kinds = [ToolKind::Cc, ToolKind::Ar, ToolKind::Ranlib]
            .into_iter()
            .collect();
        let paths = relative_paths_for(
            "x86_64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
            &kinds,
        );
        let suffix = executable_suffix();
        assert!(paths
            .iter()
            .any(|p| p == &format!("cross-bin/x86_64-unknown-linux-gnu-gcc{suffix}")));
        assert!(paths
            .iter()
            .any(|p| p == &format!("cross-bin/x86_64-linux-gnu-gcc{suffix}")));
        assert!(paths
            .iter()
            .any(|p| p == &format!("cross-bin/x86_64-linux-gnu-as{suffix}")));
        assert!(paths.iter().any(|p| p == &format!("cross-bin/ar{suffix}")));
        assert!(paths
            .iter()
            .any(|p| p == &format!("cross-bin/ranlib{suffix}")));
        assert!(!paths.iter().any(|p| p == &format!("cross-bin/gcc{suffix}")));
        assert!(!paths.iter().any(|p| p == &format!("cross-bin/cc{suffix}")));
    }

    #[test]
    fn primary_prefix_drops_unknown_vendor() {
        assert_eq!(
            primary_gnu_prefix("x86_64-unknown-linux-gnu", "x86_64-unknown-linux-gnu"),
            "x86_64-linux-gnu"
        );
        assert_eq!(
            primary_gnu_prefix("x86_64-pc-windows-gnu", "x86_64-w64-windows-gnu"),
            "x86_64-pc-windows-gnu"
        );
    }
}

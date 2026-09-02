use crate::schema::{Profile, RuntimeContract, RuntimeOwnership, SCHEMA_VERSION};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

pub const NATIVE_RCC_OWNED: &str = "native-rcc-owned";
pub const RUSTC_LINUX_MUSL_V0: &str = "rustc-linux-musl-v0";

/// Resolve a versioned runtime ownership contract for one profile.
///
/// RCC deliberately does not synthesize unknown Rust consumer contracts. Rust
/// targets differ in which CRT, libc, unwind, and linker components are
/// carried by rust-std, so accepting an unknown contract would risk duplicate
/// runtimes.
pub fn resolve(profile: &Profile, contract_id: &str) -> Result<RuntimeContract> {
    match contract_id {
        NATIVE_RCC_OWNED => native_rcc_owned(profile),
        RUSTC_LINUX_MUSL_V0 => rustc_linux_musl_v0(profile),
        _ => bail!(
            "unknown runtime contract {contract_id} for profile {}; this RCC release only \
             provides {NATIVE_RCC_OWNED} and {RUSTC_LINUX_MUSL_V0}; Rust consumer contracts \
             must be registered and validated explicitly",
            profile.profile_id
        ),
    }
}

fn native_rcc_owned(profile: &Profile) -> Result<RuntimeContract> {
    let contract = RuntimeContract {
        schema_version: SCHEMA_VERSION,
        contract_id: NATIVE_RCC_OWNED.into(),
        profile_id: profile.profile_id.clone(),
        ownership: RuntimeOwnership::RccOwned,
        consumer: None,
        consumer_target: None,
        consumer_version_requirement: None,
        component_owners: BTreeMap::new(),
        injected_link_args: Vec::new(),
        forbidden_link_args: vec![
            "-nostdlib".into(),
            "-nodefaultlibs".into(),
            "-nostartfiles".into(),
            "-nolibc".into(),
            "/nodefaultlib".into(),
        ],
    };
    contract.validate()?;
    Ok(contract)
}

fn rustc_linux_musl_v0(profile: &Profile) -> Result<RuntimeContract> {
    if profile.os != "linux" || profile.libc_family != "musl" {
        bail!(
            "{RUSTC_LINUX_MUSL_V0} is only valid for linux musl profiles, not {}",
            profile.profile_id
        );
    }

    let mut component_owners = BTreeMap::new();
    component_owners.insert("libc".into(), RuntimeOwnership::RccOwned);
    component_owners.insert("crt".into(), RuntimeOwnership::RccOwned);
    component_owners.insert("compiler-rt".into(), RuntimeOwnership::RccOwned);
    component_owners.insert("unwind".into(), RuntimeOwnership::ConsumerOwned);
    component_owners.insert("linker".into(), RuntimeOwnership::RccOwned);

    let contract = RuntimeContract {
        schema_version: SCHEMA_VERSION,
        contract_id: RUSTC_LINUX_MUSL_V0.into(),
        profile_id: profile.profile_id.clone(),
        ownership: RuntimeOwnership::SplitContract,
        consumer: Some("rustc".into()),
        consumer_target: Some(profile.target_triple.clone()),
        consumer_version_requirement: Some(">=1.85".into()),
        component_owners,
        injected_link_args: Vec::new(),
        // rustc with stable `link-self-contained=no` always passes
        // -nodefaultlibs and names libc/unwind itself. cargo-rcc exposes
        // rust-std's libunwind.a on an isolated search path; RCC still
        // injects musl CRT and compiler-rt. -nostdlib would drop those
        // driver defaults.
        forbidden_link_args: vec!["-nostdlib".into(), "-nolibc".into(), "/nodefaultlib".into()],
    };
    contract.validate()?;
    Ok(contract)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;

    #[test]
    fn native_contract_is_profile_bound_and_valid() {
        let profile = &registry::builtin_target_profiles()[0];
        let contract = resolve(profile, NATIVE_RCC_OWNED).unwrap();
        assert_eq!(contract.profile_id, profile.profile_id);
        assert_eq!(contract.ownership, RuntimeOwnership::RccOwned);
        contract.validate().unwrap();
    }

    #[test]
    fn rustc_musl_contract_is_split_and_linux_only() {
        let profile = registry::resolve_target_profile("linux-x86_64-musl-static").unwrap();
        let contract = resolve(profile, RUSTC_LINUX_MUSL_V0).unwrap();
        assert_eq!(contract.ownership, RuntimeOwnership::SplitContract);
        assert_eq!(contract.consumer.as_deref(), Some("rustc"));
        assert_eq!(
            contract.component_owners.get("libc"),
            Some(&RuntimeOwnership::RccOwned)
        );
        assert_eq!(
            contract.component_owners.get("unwind"),
            Some(&RuntimeOwnership::ConsumerOwned)
        );

        let macos = registry::resolve_target_profile("macos-aarch64").unwrap();
        let error = resolve(macos, RUSTC_LINUX_MUSL_V0).unwrap_err();
        assert!(error.to_string().contains("linux musl"));
    }

    #[test]
    fn unknown_consumer_contract_fails_closed() {
        let profile = &registry::builtin_target_profiles()[0];
        let error = resolve(profile, "rust-guessed-v0").unwrap_err();
        assert!(error.to_string().contains("must be registered"));
    }
}

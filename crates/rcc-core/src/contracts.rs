use crate::schema::{Profile, RuntimeContract, RuntimeOwnership, SCHEMA_VERSION};
use anyhow::{bail, Result};
use std::collections::BTreeMap;

pub const NATIVE_RCC_OWNED: &str = "native-rcc-owned";

/// Resolve a versioned runtime ownership contract for one profile.
///
/// RCC deliberately does not synthesize Rust consumer contracts. Rust targets
/// differ in which CRT, libc, unwind, and linker components are carried by
/// rust-std, so accepting an unknown contract would risk duplicate runtimes.
pub fn resolve(profile: &Profile, contract_id: &str) -> Result<RuntimeContract> {
    if contract_id != NATIVE_RCC_OWNED {
        bail!(
            "unknown runtime contract {contract_id} for profile {}; this RCC release only \
             provides {NATIVE_RCC_OWNED}; Rust consumer contracts must be registered and \
             validated explicitly",
            profile.profile_id
        );
    }

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
    fn unknown_consumer_contract_fails_closed() {
        let profile = &registry::builtin_target_profiles()[0];
        let error = resolve(profile, "rust-guessed-v0").unwrap_err();
        assert!(error.to_string().contains("must be registered"));
    }
}

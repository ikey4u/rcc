use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use rcc_core::{
    contracts,
    layout::{build_view_manifest, launcher_path, ControllerIdentity},
    pack::PackInspection,
    schema::{Profile, ToolKind, ViewManifest},
    view::{MaterializedView, ViewMaterializer},
};

use crate::{
    cache, engine,
    payload::{Payload, PayloadOrigin},
    provider,
};

pub const CONTROLLER_HOST: &str = env!("RCC_HOST_TRIPLE");
pub const CONTROLLER_BUILD_SHA256: &str = env!("RCC_CONTROLLER_BUILD_SHA256");

#[derive(Clone, Debug)]
pub struct ResolvedView {
    pub manifest: ViewManifest,
    pub materialized: MaterializedView,
    pub pack: PackInspection,
    pub pack_origin: String,
}

impl ResolvedView {
    pub fn launcher(&self, kind: ToolKind) -> Result<PathBuf> {
        ensure!(
            self.manifest.tools.contains_key(&kind),
            "profile {} does not provide tool kind {kind}",
            self.manifest.profile.profile_id
        );
        let path = launcher_path(&self.manifest, kind);
        let metadata = std::fs::symlink_metadata(&path).with_context(|| {
            format!("profile launcher is missing: {}", path.display())
        })?;
        ensure!(
            metadata.file_type().is_file()
                && !metadata.file_type().is_symlink(),
            "profile launcher is not a regular file: {}",
            path.display()
        );
        Ok(path)
    }
}

pub fn materialize(
    home_directory: Option<&Path>,
    cache_directory: Option<&Path>,
    external_pack: Option<&Path>,
    allow_external_pack: bool,
    profile: &Profile,
    contract_id: &str,
) -> Result<ResolvedView> {
    let cache_root = cache::resolve(cache_directory)?;
    let materializer = ViewMaterializer::new(&cache_root)?;
    ensure!(
        engine::is_available(),
        "this RCC executable has no statically integrated LLVM engine"
    );
    for kind in &profile.tool_kinds {
        ensure!(
            engine::supports(*kind),
            "static engine {} does not provide profile tool kind {kind}",
            engine::BUILD_ID
        );
    }
    let executable = std::env::current_exe()
        .context("failed to locate the running RCC binary")?;
    let controller = materializer
        .persist_controller(&executable)
        .context("failed to persist the static RCC controller")?;
    let payload = Payload::load(external_pack, allow_external_pack)?;
    let origin = describe_origin(payload.origin());
    let inspected = payload.inspection()?;
    let (pack_path, pack) = materializer
        .persist_inspected_pack_bytes(payload.bytes(), inspected)
        .context("failed to verify and persist RCC payload")?;
    ensure!(
        pack.manifest.host == CONTROLLER_HOST,
        "payload host {} cannot run on controller host {}",
        pack.manifest.host,
        CONTROLLER_HOST
    );

    let contract = contracts::resolve(profile, contract_id)?;
    let external_sysroot =
        provider::resolve_external_sysroot(profile, home_directory)?;
    let controller_identity = ControllerIdentity::new(
        CONTROLLER_BUILD_SHA256,
        &controller,
        engine::BUILD_ID,
    );
    let manifest = build_view_manifest(
        &materializer,
        &pack,
        profile,
        &contract,
        &controller_identity,
        external_sysroot.as_ref(),
    )?;
    let materialized = materializer
        .materialize_verified(&pack_path, &pack, &controller, &manifest)
        .context("failed to materialize immutable toolchain view")?;

    Ok(ResolvedView {
        manifest,
        materialized,
        pack,
        pack_origin: origin,
    })
}

fn describe_origin(origin: &PayloadOrigin) -> String {
    match origin {
        PayloadOrigin::Embedded => "embedded".into(),
        PayloadOrigin::External(path) => format!("external:{}", path.display()),
    }
}

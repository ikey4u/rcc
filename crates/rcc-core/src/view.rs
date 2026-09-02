use crate::digest::file_sha256;
use crate::pack::{
    create_unique_dir, extract_pack_into, inspect_embedded_pack_bytes, inspect_pack_bytes,
    verify_directory_metadata_with_extras, verify_pack, DirectoryCleanup, PackInspection,
};
use crate::schema::ViewManifest;
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const VIEW_MANIFEST_FILE: &str = "view.json";

#[derive(Clone, Debug)]
pub struct ViewMaterializer {
    cache_root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedView {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
    pub pack_sha256: String,
    pub reused: bool,
}

/// A verified, immutable controller executable persisted below
/// `cache/controllers/<sha256>/rcc`. The fields are intentionally private so
/// callers cannot manufacture an unverified controller capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerExecutable {
    path: PathBuf,
    sha256: String,
    size: u64,
}

impl ControllerExecutable {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

impl ViewMaterializer {
    /// Creates a materializer rooted at an absolute, canonical cache path.
    pub fn new(cache_root: &Path) -> Result<Self> {
        prepare_directory(cache_root, "cache root")?;
        let cache_root = cache_root.canonicalize().with_context(|| {
            format!("failed to canonicalize cache root {}", cache_root.display())
        })?;
        for child in ["blobs", "controllers", "locks", "tmp", "views"] {
            prepare_directory(&cache_root.join(child), "cache directory")?;
        }
        Ok(Self { cache_root })
    }

    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Computes the stable location a caller must use when constructing the
    /// absolute paths in a `ViewManifest`.
    pub fn view_root(
        &self,
        identity: &str,
        profile_id: &str,
        contract_id: &str,
    ) -> Result<PathBuf> {
        ensure!(
            is_lower_sha256(identity),
            "view identity is not a canonical SHA-256"
        );
        validate_id_component("profile id", profile_id)?;
        validate_id_component("runtime contract id", contract_id)?;
        Ok(self
            .cache_root
            .join("views")
            .join(identity)
            .join(profile_id)
            .join(contract_id))
    }

    /// Materializes `pack_path` as the immutable file tree described by
    /// `view_manifest`.
    ///
    /// A per-view cross-process lock serializes validation and publication.
    /// Extraction and manifest creation happen below `cache/tmp`, followed by
    /// an atomic rename. An invalid existing view is moved to an identifiable
    /// quarantine directory and rebuilt from the verified pack.
    pub fn materialize(
        &self,
        pack_path: &Path,
        controller: &ControllerExecutable,
        view_manifest: &ViewManifest,
    ) -> Result<MaterializedView> {
        let pack = verify_pack(pack_path)?;
        self.materialize_verified(pack_path, &pack, controller, view_manifest)
    }

    /// Materialize using a pack inspection already verified by the caller.
    /// This avoids decompressing every pack member again after an embedded
    /// payload has been inspected and persisted under its content digest.
    pub fn materialize_verified(
        &self,
        pack_path: &Path,
        pack: &PackInspection,
        controller: &ControllerExecutable,
        view_manifest: &ViewManifest,
    ) -> Result<MaterializedView> {
        view_manifest.validate().context("invalid view manifest")?;
        crate::layout::validate_view_binding(view_manifest)
            .context("invalid view identity binding")?;
        let target = self.view_root(
            &view_manifest.identity,
            &view_manifest.profile.profile_id,
            &view_manifest.runtime_contract.contract_id,
        )?;
        let declared_root = Path::new(&view_manifest.root);
        ensure!(
            declared_root == target,
            "view manifest root {} does not match materialization path {}",
            declared_root.display(),
            target.display()
        );

        ensure!(
            pack.sha256 == view_manifest.pack_sha256,
            "view manifest expects pack {}, not {}",
            view_manifest.pack_sha256,
            pack.sha256
        );
        ensure_resource_pack_paths(&pack.manifest.files)?;
        self.validate_controller(controller, &view_manifest.controller_sha256)?;

        let lock_path = self.cache_root.join("locks").join(format!(
            "{}.{}.{}.lock",
            view_manifest.identity,
            view_manifest.profile.profile_id,
            view_manifest.runtime_contract.contract_id
        ));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open view lock {}", lock_path.display()))?;
        FileExt::lock_exclusive(&lock)
            .with_context(|| format!("failed to lock view {}", target.display()))?;

        let result = self.materialize_locked(pack_path, pack, controller, view_manifest, &target);
        FileExt::unlock(&lock)
            .with_context(|| format!("failed to unlock view {}", target.display()))?;
        result
    }

    /// Persists an embedded pack into the content-addressed blob cache and
    /// materializes it. This is the bridge used by a single-file controller's
    /// `include_bytes!` payload; callers do not need to invent a temporary
    /// external pack path.
    pub fn materialize_bytes(
        &self,
        pack_bytes: &[u8],
        controller: &ControllerExecutable,
        view_manifest: &ViewManifest,
    ) -> Result<MaterializedView> {
        let (blob, pack) = self.persist_pack_bytes(pack_bytes)?;
        self.materialize_verified(&blob, &pack, controller, view_manifest)
    }

    /// Copy the running RCC executable into a content-addressed controller
    /// cache. Views hardlink this immutable cache entry instead of embedding
    /// or copying separate compiler/linker programs.
    pub fn persist_controller(&self, executable_path: &Path) -> Result<ControllerExecutable> {
        let source = executable_path.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize controller executable {}",
                executable_path.display()
            )
        })?;
        let source_metadata = fs::symlink_metadata(&source).with_context(|| {
            format!(
                "failed to inspect controller executable {}",
                source.display()
            )
        })?;
        ensure!(
            source_metadata.file_type().is_file() && !source_metadata.file_type().is_symlink(),
            "controller executable is not a regular file: {}",
            source.display()
        );
        ensure!(
            source_metadata.len() != 0,
            "controller executable is empty: {}",
            source.display()
        );
        ensure_source_executable(&source_metadata, &source)?;
        let digest = file_sha256(&source).with_context(|| {
            format!("failed to hash controller executable {}", source.display())
        })?;
        let target_directory = self.cache_root.join("controllers").join(&digest);
        let target = target_directory.join("rcc");
        let lock_path = self
            .cache_root
            .join("locks")
            .join(format!("controller.{digest}.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open controller lock {}", lock_path.display()))?;
        FileExt::lock_exclusive(&lock)
            .with_context(|| format!("failed to lock controller {digest}"))?;

        let persist_result = (|| -> Result<()> {
            if path_exists(&target_directory) {
                if cached_controller_is_valid(&target, &digest, source_metadata.len()) {
                    return Ok(());
                }
                let quarantine =
                    create_unique_dir(&self.cache_root.join("tmp"), "corrupt-controller")?;
                fs::rename(&target_directory, quarantine.join("controller")).with_context(
                    || {
                        format!(
                            "failed to quarantine invalid controller {}",
                            target_directory.display()
                        )
                    },
                )?;
            }

            let temporary = create_unique_dir(&self.cache_root.join("tmp"), ".controller")?;
            let mut cleanup = DirectoryCleanup::new(temporary.clone());
            let staged = temporary.join("rcc");
            fs::copy(&source, &staged).with_context(|| {
                format!(
                    "failed to stage controller executable {} as {}",
                    source.display(),
                    staged.display()
                )
            })?;
            ensure!(
                fs::metadata(&staged)?.len() == source_metadata.len(),
                "staged controller size changed while copying"
            );
            ensure!(
                file_sha256(&staged)? == digest,
                "controller executable changed while being persisted"
            );
            set_file_read_only(&staged, true)?;
            File::open(&staged)?
                .sync_all()
                .context("failed to sync staged controller executable")?;
            ensure!(
                !path_exists(&target_directory),
                "controller cache entry appeared while publishing"
            );
            fs::rename(&temporary, &target_directory).with_context(|| {
                format!(
                    "failed to atomically publish controller {} as {}",
                    temporary.display(),
                    target_directory.display()
                )
            })?;
            cleanup.disarm();
            Ok(())
        })();
        FileExt::unlock(&lock).context("failed to unlock controller executable")?;
        persist_result?;

        let controller = ControllerExecutable {
            path: target,
            sha256: digest,
            size: source_metadata.len(),
        };
        self.validate_controller(&controller, &controller.sha256)?;
        Ok(controller)
    }

    /// Persist and verify an in-memory pack without materializing a profile
    /// view. The returned inspection is used to derive the view identity and
    /// absolute manifest paths before extraction.
    pub fn persist_pack_bytes(&self, pack_bytes: &[u8]) -> Result<(PathBuf, PackInspection)> {
        ensure!(!pack_bytes.is_empty(), "embedded pack is empty");
        let inspection =
            inspect_pack_bytes(pack_bytes).context("embedded pack failed inspection")?;
        self.persist_inspected_pack_bytes(pack_bytes, inspection)
    }

    /// Persist bytes paired with an inspection already established from those
    /// exact bytes. Runtime-provided packs must use `persist_pack_bytes`.
    pub fn persist_inspected_pack_bytes(
        &self,
        pack_bytes: &[u8],
        inspection: PackInspection,
    ) -> Result<(PathBuf, PackInspection)> {
        ensure!(!pack_bytes.is_empty(), "embedded pack is empty");
        ensure!(
            inspection.pack_size == pack_bytes.len() as u64,
            "pack inspection size does not match embedded bytes"
        );
        ensure!(
            is_lower_sha256(&inspection.sha256),
            "pack inspection digest is not canonical"
        );
        ensure!(
            inspect_embedded_pack_bytes(pack_bytes, &inspection.sha256)? == inspection,
            "pack inspection does not describe the supplied bytes"
        );
        let digest = inspection.sha256.clone();
        let blob = self
            .cache_root
            .join("blobs")
            .join(format!("{digest}.rccpack"));
        let lock_path = self
            .cache_root
            .join("locks")
            .join(format!("blob.{digest}.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open blob lock {}", lock_path.display()))?;
        FileExt::lock_exclusive(&lock)
            .with_context(|| format!("failed to lock payload blob {digest}"))?;

        let persist_result = (|| -> Result<()> {
            if path_exists(&blob) {
                let valid = fs::symlink_metadata(&blob)
                    .map(|metadata| metadata.file_type().is_file())
                    .unwrap_or(false)
                    && file_sha256(&blob).is_ok_and(|actual| actual == digest);
                if valid {
                    return Ok(());
                }
                let quarantine = create_unique_dir(&self.cache_root.join("tmp"), "corrupt-blob")?;
                fs::rename(&blob, quarantine.join("payload.rccpack")).with_context(|| {
                    format!(
                        "failed to quarantine invalid payload blob {}",
                        blob.display()
                    )
                })?;
            }

            let temporary = create_unique_dir(&self.cache_root.join("tmp"), ".blob")?;
            let _cleanup = DirectoryCleanup::new(temporary.clone());
            let staged = temporary.join("payload.rccpack");
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)
                .with_context(|| format!("failed to stage embedded pack {}", staged.display()))?;
            file.write_all(pack_bytes)
                .context("failed to write embedded pack into blob cache")?;
            file.sync_all()
                .context("failed to sync embedded pack in blob cache")?;
            drop(file);
            ensure!(
                file_sha256(&staged)? == digest,
                "staged embedded pack digest changed while writing"
            );
            ensure!(
                !path_exists(&blob),
                "payload blob appeared while publishing"
            );
            fs::rename(&staged, &blob).with_context(|| {
                format!(
                    "failed to atomically publish payload blob {} as {}",
                    staged.display(),
                    blob.display()
                )
            })?;
            set_file_read_only(&blob, false)?;
            Ok(())
        })();
        FileExt::unlock(&lock).context("failed to unlock payload blob")?;
        persist_result?;
        Ok((blob, inspection))
    }

    pub(crate) fn validate_controller(
        &self,
        controller: &ControllerExecutable,
        expected_sha256: &str,
    ) -> Result<()> {
        ensure!(
            controller.sha256 == expected_sha256,
            "view expects controller {}, not {}",
            expected_sha256,
            controller.sha256
        );
        ensure!(
            is_lower_sha256(&controller.sha256),
            "controller digest is not canonical"
        );
        let expected_path = self
            .cache_root
            .join("controllers")
            .join(&controller.sha256)
            .join("rcc");
        ensure!(
            controller.path == expected_path,
            "controller path {} is not its content-addressed cache location {}",
            controller.path.display(),
            expected_path.display()
        );
        ensure!(
            cached_controller_metadata_is_valid(&controller.path, controller.size),
            "cached controller {} failed type, size, or mode validation",
            controller.path.display()
        );
        Ok(())
    }

    fn materialize_locked(
        &self,
        pack_path: &Path,
        pack: &PackInspection,
        controller: &ControllerExecutable,
        view_manifest: &ViewManifest,
        target: &Path,
    ) -> Result<MaterializedView> {
        if path_exists(target) {
            match validate_materialized_view(target, &pack.manifest, controller, view_manifest) {
                Ok(()) => {
                    seal_view(target, &pack.manifest)?;
                    return Ok(materialized_result(target, pack, true));
                }
                Err(error) => self
                    .quarantine(target)
                    .with_context(|| format!("existing view was invalid ({error:#})"))?,
            }
        }

        let temporary = create_unique_dir(&self.cache_root.join("tmp"), ".view")?;
        let mut cleanup = DirectoryCleanup::new(temporary.clone());
        let extracted = extract_pack_into(pack_path, &temporary)?;
        ensure!(
            extracted.sha256 == pack.sha256 && extracted.manifest == pack.manifest,
            "pack changed while the view was being materialized"
        );
        create_launcher_aliases(&temporary, controller, view_manifest)?;
        write_view_manifest(&temporary, view_manifest)?;
        validate_materialized_view(&temporary, &pack.manifest, controller, view_manifest)?;
        // Seal every member before publication. macOS refuses to rename a
        // directory whose root itself is mode 0555, so the root stays writable
        // only until the atomic rename below. Its final read-only mode is the
        // readiness marker checked by every multicall alias before dispatch.
        seal_view_contents(&temporary, &pack.manifest)?;

        let parent = target.parent().context("view target has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create view parent {}", parent.display()))?;
        ensure!(
            !path_exists(target),
            "view target appeared while materializing: {}",
            target.display()
        );
        if let Err(error) = fs::rename(&temporary, target) {
            let _ = make_tree_writable(&temporary);
            return Err(error).with_context(|| {
                format!(
                    "failed to atomically publish view {} as {}",
                    temporary.display(),
                    target.display()
                )
            });
        }
        cleanup.disarm();
        set_directory_read_only(target)?;
        validate_materialized_view(target, &pack.manifest, controller, view_manifest)?;
        Ok(materialized_result(target, pack, false))
    }

    fn quarantine(&self, target: &Path) -> Result<()> {
        let quarantine = create_unique_dir(&self.cache_root.join("tmp"), "corrupt-view")?;
        let quarantined_view = quarantine.join("view");
        let metadata = fs::symlink_metadata(target)
            .with_context(|| format!("failed to inspect invalid view {}", target.display()))?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            set_directory_writable(target)?;
        }
        fs::rename(target, &quarantined_view).with_context(|| {
            format!(
                "failed to quarantine invalid view {} as {}",
                target.display(),
                quarantined_view.display()
            )
        })
    }
}

pub fn materialize_view(
    pack_path: &Path,
    cache_root: &Path,
    controller_path: &Path,
    view_manifest: &ViewManifest,
) -> Result<MaterializedView> {
    let materializer = ViewMaterializer::new(cache_root)?;
    let controller = materializer.persist_controller(controller_path)?;
    materializer.materialize(pack_path, &controller, view_manifest)
}

fn ensure_resource_pack_paths(files: &[crate::schema::PackFile]) -> Result<()> {
    for file in files {
        ensure!(
            file.path != "bin"
                && !file.path.starts_with("bin/")
                && file.path != "launchers"
                && !file.path.starts_with("launchers/")
                && file.path != VIEW_MANIFEST_FILE
                && !file.path.starts_with(&format!("{VIEW_MANIFEST_FILE}/")),
            "resource pack contains reserved executable/view path {}",
            file.path
        );
    }
    Ok(())
}

fn launcher_relative_paths(view: &ViewManifest) -> Result<Vec<String>> {
    let root = Path::new(&view.root);
    view.tools
        .values()
        .map(|tool| {
            let relative = Path::new(&tool.path).strip_prefix(root).with_context(|| {
                format!("view tool path {} escapes {}", tool.path, root.display())
            })?;
            let mut components = Vec::new();
            for component in relative.components() {
                match component {
                    std::path::Component::Normal(value) => {
                        components.push(value.to_string_lossy().into_owned())
                    }
                    _ => {
                        return Err(anyhow::anyhow!(
                            "view tool path is not normalized: {}",
                            tool.path
                        ))
                    }
                }
            }
            ensure!(
                !components.is_empty(),
                "view tool path has no relative name"
            );
            Ok(components.join("/"))
        })
        .collect()
}

fn create_launcher_aliases(
    root: &Path,
    controller: &ControllerExecutable,
    view: &ViewManifest,
) -> Result<()> {
    let launcher_directory = root.join("launchers");
    fs::create_dir(&launcher_directory).with_context(|| {
        format!(
            "failed to create launcher directory {}",
            launcher_directory.display()
        )
    })?;
    for relative in launcher_relative_paths(view)? {
        let alias = root.join(&relative);
        fs::hard_link(&controller.path, &alias).with_context(|| {
            format!(
                "failed to hardlink controller {} as {}",
                controller.path.display(),
                alias.display()
            )
        })?;
        let metadata = fs::symlink_metadata(&alias)
            .with_context(|| format!("failed to inspect launcher alias {}", alias.display()))?;
        ensure_cached_executable_mode(&metadata, &alias)?;
        verify_controller_alias(&alias, controller)?;
    }
    Ok(())
}

fn cached_controller_is_valid(path: &Path, digest: &str, size: u64) -> bool {
    cached_controller_metadata_is_valid(path, size)
        && file_sha256(path).is_ok_and(|actual| actual == digest)
}

fn cached_controller_metadata_is_valid(path: &Path, size: u64) -> bool {
    let Some(directory) = path.parent() else {
        return false;
    };
    let Ok(directory_metadata) = fs::symlink_metadata(directory) else {
        return false;
    };
    if !directory_metadata.file_type().is_dir() || directory_metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(mut entries) = fs::read_dir(directory) else {
        return false;
    };
    let Some(Ok(entry)) = entries.next() else {
        return false;
    };
    if entry.file_name() != "rcc" || entries.next().is_some() {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && metadata.len() == size
        && cached_executable_mode_is_valid(&metadata)
}

fn ensure_source_executable(metadata: &fs::Metadata, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o111 != 0,
            "controller source is not executable: {}",
            path.display()
        );
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, path);
    }
    Ok(())
}

fn cached_executable_mode_is_valid(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777 == 0o555
    }
    #[cfg(not(unix))]
    {
        metadata.permissions().readonly()
    }
}

fn ensure_cached_executable_mode(metadata: &fs::Metadata, path: &Path) -> Result<()> {
    ensure!(
        cached_executable_mode_is_valid(metadata),
        "controller executable or alias has an invalid mode: {}",
        path.display()
    );
    Ok(())
}

fn verify_controller_alias(alias: &Path, controller: &ControllerExecutable) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let alias_metadata = fs::metadata(alias)?;
        let controller_metadata = fs::metadata(&controller.path)?;
        ensure!(
            alias_metadata.dev() == controller_metadata.dev()
                && alias_metadata.ino() == controller_metadata.ino(),
            "launcher alias {} is not a hardlink of controller {}",
            alias.display(),
            controller.path.display()
        );
    }
    #[cfg(not(unix))]
    {
        let digest = file_sha256(alias)?;
        ensure!(
            digest == controller.sha256,
            "launcher alias {} digest mismatch: expected {}, got {}",
            alias.display(),
            controller.sha256,
            digest
        );
    }
    Ok(())
}

fn materialized_result(target: &Path, pack: &PackInspection, reused: bool) -> MaterializedView {
    MaterializedView {
        root: target.to_owned(),
        manifest_path: target.join(VIEW_MANIFEST_FILE),
        pack_sha256: pack.sha256.clone(),
        reused,
    }
}

fn write_view_manifest(root: &Path, manifest: &ViewManifest) -> Result<()> {
    let bytes = serde_json::to_vec(manifest).context("failed to encode view manifest")?;
    let path = root.join(VIEW_MANIFEST_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("failed to create view manifest {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("failed to write view manifest {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync view manifest {}", path.display()))
}

fn validate_materialized_view(
    actual_root: &Path,
    pack_manifest: &crate::schema::PackManifest,
    controller: &ControllerExecutable,
    expected_view: &ViewManifest,
) -> Result<()> {
    let mut extra_files = vec![VIEW_MANIFEST_FILE.to_owned()];
    extra_files.extend(launcher_relative_paths(expected_view)?);
    let extra_file_refs = extra_files.iter().map(String::as_str).collect::<Vec<_>>();
    verify_directory_metadata_with_extras(pack_manifest, actual_root, &extra_file_refs)?;
    let manifest_path = actual_root.join(VIEW_MANIFEST_FILE);
    let bytes = fs::read(&manifest_path)
        .with_context(|| format!("failed to read view manifest {}", manifest_path.display()))?;
    let actual: ViewManifest =
        serde_json::from_slice(&bytes).context("failed to decode materialized view manifest")?;
    actual
        .validate()
        .context("materialized view manifest is invalid")?;
    ensure!(
        &actual == expected_view,
        "materialized view manifest does not match the requested view"
    );

    for file in pack_manifest.files.iter().filter(|file| file.executable) {
        let path = actual_root.join(&file.path);
        let digest = file_sha256(&path)?;
        ensure!(
            digest == file.sha256,
            "view executable digest mismatch for {}: expected {}, got {}",
            file.path,
            file.sha256,
            digest
        );
    }

    for (kind, tool) in &actual.tools {
        let path = map_declared_path(&actual.root, actual_root, &tool.path)?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("view tool {kind} is missing at {}", path.display()))?;
        ensure!(
            metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
            "view tool {kind} is not a regular file: {}",
            path.display()
        );
        ensure!(
            metadata.len() == controller.size,
            "view tool {kind} size mismatch: expected {}, got {}",
            controller.size,
            metadata.len()
        );
        ensure_cached_executable_mode(&metadata, &path)?;
        ensure!(
            tool.sha256 == controller.sha256,
            "view tool {kind} digest binding mismatch: expected {}, got {}",
            controller.sha256,
            tool.sha256
        );
        verify_controller_alias(&path, controller)?;
    }

    for (label, declared) in [
        ("sysroot", actual.sysroot.as_str()),
        ("resource directory", actual.resource_dir.as_str()),
    ] {
        let path = map_declared_path(&actual.root, actual_root, declared)?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("view {label} is missing at {}", path.display()))?;
        ensure!(
            metadata.file_type().is_dir() && !metadata.file_type().is_symlink(),
            "view {label} is not a directory: {}",
            path.display()
        );
    }
    Ok(())
}

fn map_declared_path(declared_root: &str, actual_root: &Path, declared: &str) -> Result<PathBuf> {
    let declared_root = Path::new(declared_root);
    let declared = Path::new(declared);
    if let Ok(relative) = declared.strip_prefix(declared_root) {
        ensure!(
            relative
                .components()
                .all(|component| { matches!(component, std::path::Component::Normal(_)) })
                || relative.as_os_str().is_empty(),
            "declared view path is not normalized: {}",
            declared.display()
        );
        return Ok(actual_root.join(relative));
    }
    Ok(declared.to_owned())
}

fn seal_view(root: &Path, pack_manifest: &crate::schema::PackManifest) -> Result<()> {
    seal_view_contents(root, pack_manifest)?;
    set_directory_read_only(root)
}

fn seal_view_contents(root: &Path, pack_manifest: &crate::schema::PackManifest) -> Result<()> {
    for file in &pack_manifest.files {
        set_file_read_only(&root.join(&file.path), file.executable)?;
    }
    set_file_read_only(&root.join(VIEW_MANIFEST_FILE), false)?;

    let mut directories = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|entry| entry.file_type().is_dir() && entry.path() != root)
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        set_directory_read_only(&directory)?;
    }
    Ok(())
}

fn set_file_read_only(path: &Path, executable: bool) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if executable { 0o555 } else { 0o444 });
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        permissions.set_readonly(true);
    }
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to seal file {}", path.display()))
}

fn set_directory_read_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o555))
            .with_context(|| format!("failed to seal directory {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn set_directory_writable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to unseal directory {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn make_tree_writable(root: &Path) -> Result<()> {
    for entry in WalkDir::new(root).follow_links(false).contents_first(true) {
        let entry = entry?;
        // Launcher files are hardlinks of the shared controller inode. Never
        // chmod a file while cleaning a view: doing so would also mutate the
        // controller cache entry and every other view alias. On Unix only the
        // containing directory needs to be writable for removal.
        if !entry.file_type().is_dir() {
            continue;
        }
        let mut permissions = entry.metadata()?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(entry.path(), permissions)?;
    }
    Ok(())
}

fn prepare_directory(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "{label} is a symlink: {}",
                path.display()
            );
            ensure!(
                metadata.is_dir(),
                "{label} is not a directory: {}",
                path.display()
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create {label} {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {label} {}", path.display()))
        }
    }
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_id_component(label: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            }),
        "invalid {label}: {value}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::bytes_sha256;
    use crate::pack::{create_pack, PackOptions};
    use crate::registry;
    use crate::schema::{RuntimeContract, RuntimeOwnership, ToolKind, ViewTool, SCHEMA_VERSION};
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;
    use tempfile::tempdir;

    struct Fixture {
        pack: PathBuf,
        materializer: ViewMaterializer,
        controller: ControllerExecutable,
        controller_source: PathBuf,
        view: ViewManifest,
    }

    fn fixture(temporary: &Path) -> Fixture {
        let source = temporary.join("source");
        fs::create_dir_all(source.join("sysroot")).unwrap();
        fs::create_dir_all(source.join("lib/clang/22")).unwrap();
        fs::create_dir_all(source.join("lib/c++/v1")).unwrap();
        fs::write(source.join("sysroot/.keep"), b"sysroot").unwrap();
        fs::write(source.join("lib/clang/22/.keep"), b"resource").unwrap();
        fs::write(source.join("lib/c++/v1/.keep"), b"cxx").unwrap();

        let profile = registry::builtin_target_profiles()[0].clone();
        let pack = temporary.join("fixture.rccpack");
        create_pack(
            &source,
            &pack,
            &PackOptions::new(
                "fixture-pack",
                "r1",
                "aarch64-apple-darwin",
                [profile.profile_id.as_str()],
            ),
        )
        .unwrap();
        let materializer = ViewMaterializer::new(&temporary.join("cache")).unwrap();
        let controller_source = temporary.join("rcc-controller");
        fs::write(&controller_source, b"fixture statically linked controller").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&controller_source, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let controller = materializer.persist_controller(&controller_source).unwrap();
        let identity = bytes_sha256(b"fixture-view");
        let contract = RuntimeContract {
            schema_version: SCHEMA_VERSION,
            contract_id: "native-rcc-owned".into(),
            profile_id: profile.profile_id.clone(),
            ownership: RuntimeOwnership::RccOwned,
            consumer: None,
            consumer_target: None,
            consumer_version_requirement: None,
            component_owners: BTreeMap::new(),
            injected_link_args: Vec::new(),
            forbidden_link_args: Vec::new(),
        };
        let root = materializer
            .view_root(&identity, &profile.profile_id, &contract.contract_id)
            .unwrap();
        let tools = profile
            .tool_kinds
            .iter()
            .map(|kind| {
                let driver_kind = match kind {
                    ToolKind::Cc | ToolKind::Cxx => Some(profile.driver_kind),
                    ToolKind::Linker
                        if profile.linker_flavor == crate::schema::LinkerFlavor::CoffMsvc =>
                    {
                        Some(crate::schema::DriverKind::LldLink)
                    }
                    _ => None,
                };
                (
                    *kind,
                    ViewTool {
                        path: root
                            .join("launchers")
                            .join(crate::schema::launcher_name(&profile, *kind))
                            .to_string_lossy()
                            .into_owned(),
                        sha256: controller.sha256.clone(),
                        driver_kind,
                    },
                )
            })
            .collect();
        let mut view = ViewManifest {
            schema_version: SCHEMA_VERSION,
            identity,
            controller_build_sha256: "33".repeat(32),
            controller_sha256: controller.sha256.clone(),
            engine_build_id: "llvm-test-engine".into(),
            pack_sha256: bytes_sha256(&fs::read(&pack).unwrap()),
            external_sysroot_identity: None,
            profile,
            runtime_contract: contract,
            root: root.to_string_lossy().into_owned(),
            tools,
            sysroot: root.join("sysroot").to_string_lossy().into_owned(),
            resource_dir: root.join("lib/clang/22").to_string_lossy().into_owned(),
            injected_args: BTreeMap::new(),
            forbidden_env: BTreeSet::new(),
        };
        let bound_identity = crate::layout::recompute_view_identity(&view).unwrap();
        let bound_root = materializer
            .view_root(
                &bound_identity,
                &view.profile.profile_id,
                &view.runtime_contract.contract_id,
            )
            .unwrap();
        for tool in view.tools.values_mut() {
            let relative = Path::new(&tool.path).strip_prefix(&root).unwrap();
            tool.path = bound_root.join(relative).to_string_lossy().into_owned();
        }
        view.sysroot = bound_root.join("sysroot").to_string_lossy().into_owned();
        view.resource_dir = bound_root
            .join("lib/clang/22")
            .to_string_lossy()
            .into_owned();
        view.identity = bound_identity;
        view.root = bound_root.to_string_lossy().into_owned();
        crate::layout::validate_view_binding(&view).unwrap();
        Fixture {
            pack,
            materializer,
            controller,
            controller_source,
            view,
        }
    }

    #[test]
    fn materializes_reuses_and_repairs_a_view() {
        let temporary = tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let pack_bytes = fs::read(&fixture.pack).unwrap();
        let first = fixture
            .materializer
            .materialize_bytes(&pack_bytes, &fixture.controller, &fixture.view)
            .unwrap();
        assert!(!first.reused);
        assert!(first.manifest_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&first.root).unwrap().permissions().mode() & 0o222,
                0
            );
        }

        let second = fixture
            .materializer
            .materialize_bytes(&pack_bytes, &fixture.controller, &fixture.view)
            .unwrap();
        assert!(second.reused);
        assert_eq!(first.root, second.root);

        for tool in fixture.view.tools.values() {
            assert_eq!(
                file_sha256(Path::new(&tool.path)).unwrap(),
                fixture.controller.sha256
            );
            verify_controller_alias(Path::new(&tool.path), &fixture.controller).unwrap();
        }

        let resource = first.root.join("lib/clang/22/.keep");
        let mut permissions = fs::metadata(&resource).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o644);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        fs::set_permissions(&resource, permissions).unwrap();
        fs::write(&resource, b"x").unwrap();
        let repaired = fixture
            .materializer
            .materialize_bytes(&pack_bytes, &fixture.controller, &fixture.view)
            .unwrap();
        assert!(!repaired.reused);
        assert_eq!(
            fs::read(repaired.root.join("lib/clang/22/.keep")).unwrap(),
            b"resource"
        );
        assert!(cached_controller_is_valid(
            fixture.controller.path(),
            fixture.controller.sha256(),
            fixture.controller.size()
        ));
    }

    #[test]
    fn concurrent_materialization_publishes_one_valid_view() {
        let temporary = tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let pack = Arc::new(fixture.pack);
        let materializer = Arc::new(fixture.materializer);
        let controller = Arc::new(fixture.controller);
        let view = Arc::new(fixture.view);
        let mut threads = Vec::new();
        for _ in 0..4 {
            let pack = Arc::clone(&pack);
            let materializer = Arc::clone(&materializer);
            let controller = Arc::clone(&controller);
            let view = Arc::clone(&view);
            threads.push(std::thread::spawn(move || {
                materializer
                    .materialize(pack.as_path(), &controller, &view)
                    .unwrap()
            }));
        }
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| !result.reused).count(), 1);
        assert!(results[0].root.join(VIEW_MANIFEST_FILE).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn repairs_controller_cache_without_chmodding_shared_old_inode() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let pack_bytes = fs::read(&fixture.pack).unwrap();
        fixture
            .materializer
            .materialize_bytes(&pack_bytes, &fixture.controller, &fixture.view)
            .unwrap();
        let old_alias = PathBuf::from(&fixture.view.tools[&ToolKind::Cc].path);
        fs::set_permissions(&old_alias, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!cached_executable_mode_is_valid(
            &fs::metadata(fixture.controller.path()).unwrap()
        ));

        let repaired_controller = fixture
            .materializer
            .persist_controller(&fixture.controller_source)
            .unwrap();
        assert!(cached_executable_mode_is_valid(
            &fs::metadata(repaired_controller.path()).unwrap()
        ));
        assert!(!same_unix_file(&old_alias, repaired_controller.path()).unwrap());

        let repaired_view = fixture
            .materializer
            .materialize_bytes(&pack_bytes, &repaired_controller, &fixture.view)
            .unwrap();
        assert!(!repaired_view.reused);
        let new_alias = PathBuf::from(&fixture.view.tools[&ToolKind::Cc].path);
        assert!(same_unix_file(&new_alias, repaired_controller.path()).unwrap());
        assert_eq!(
            fs::metadata(repaired_controller.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o555
        );
    }

    #[cfg(unix)]
    #[test]
    fn replaces_a_symlinked_controller_cache_entry() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temporary = tempdir().unwrap();
        let materializer = ViewMaterializer::new(&temporary.path().join("cache")).unwrap();
        let source = temporary.path().join("rcc-source");
        fs::write(&source, b"trusted controller bytes").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        let digest = file_sha256(&source).unwrap();

        let external = temporary.path().join("external-controller");
        fs::create_dir(&external).unwrap();
        fs::copy(&source, external.join("rcc")).unwrap();
        fs::set_permissions(external.join("rcc"), fs::Permissions::from_mode(0o555)).unwrap();
        symlink(
            &external,
            materializer.cache_root().join("controllers").join(&digest),
        )
        .unwrap();

        let controller = materializer.persist_controller(&source).unwrap();
        let directory_metadata = fs::symlink_metadata(controller.path().parent().unwrap()).unwrap();
        assert!(directory_metadata.is_dir());
        assert!(!directory_metadata.file_type().is_symlink());
        assert!(cached_controller_is_valid(
            controller.path(),
            controller.sha256(),
            controller.size()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn quarantining_a_symlinked_view_does_not_chmod_its_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temporary = tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let external = temporary.path().join("external-view");
        fs::create_dir(&external).unwrap();
        fs::set_permissions(&external, fs::Permissions::from_mode(0o500)).unwrap();
        let target = PathBuf::from(&fixture.view.root);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(&external, &target).unwrap();

        fixture
            .materializer
            .materialize(&fixture.pack, &fixture.controller, &fixture.view)
            .unwrap();
        assert_eq!(
            fs::metadata(&external).unwrap().permissions().mode() & 0o777,
            0o500
        );
    }

    #[test]
    fn rejects_a_manifest_for_a_different_cache_root() {
        let temporary = tempdir().unwrap();
        let fixture = fixture(temporary.path());
        let mut view = fixture.view;
        view.root = "/wrong/root".into();
        assert!(fixture
            .materializer
            .materialize(&fixture.pack, &fixture.controller, &view)
            .is_err());
    }

    #[cfg(unix)]
    fn same_unix_file(left: &Path, right: &Path) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;
        let left = fs::metadata(left)?;
        let right = fs::metadata(right)?;
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }
}

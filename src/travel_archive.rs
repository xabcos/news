//! A private, independently verifiable second copy of each confirmed identity.
//!
//! An archive is published as one complete directory. Later calls verify it;
//! they never update an existing person's identity or overwrite their originals.
use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

const MANIFEST: &str = "manifest.json";
const MANIFEST_HASH: &str = "manifest.sha256";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct ArchiveAsset {
    pub id: Uuid,
    pub role: String,
    pub ordinal: i16,
    pub content_type: String,
    pub image_content_type: String,
    pub original_bytes: Vec<u8>,
    pub image_bytes: Vec<u8>,
    pub sha256: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    companion_id: Uuid,
    identity: Value,
    accessories: Value,
    assets: Vec<AssetManifest>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct AssetManifest {
    id: Uuid,
    role: String,
    ordinal: i16,
    content_type: String,
    image_content_type: String,
    original_file: String,
    image_file: String,
    sha256: String,
    image_sha256: String,
    original_size: u64,
    image_size: u64,
}

pub async fn write_identity_archive(
    directory: &Path,
    companion_id: Uuid,
    identity: &Value,
    accessories: &Value,
    assets: &[ArchiveAsset],
) -> Result<()> {
    let directory = directory.to_path_buf();
    let identity = identity.clone();
    let accessories = accessories.clone();
    let assets = assets.to_vec();
    tokio::task::spawn_blocking(move || {
        write_archive(&directory, companion_id, &identity, &accessories, &assets)
    })
    .await
    .map_err(|_| invalid("档案写入任务未完成"))?
}

pub async fn verify_identity_archive(directory: &Path, companion_id: Uuid) -> Result<()> {
    let directory = directory.to_path_buf();
    tokio::task::spawn_blocking(move || {
        check_directory_chain(&directory)?;
        require_private_directory(&directory)?;
        read_verified_archive(&directory.join(companion_id.to_string()), companion_id).map(|_| ())
    })
    .await
    .map_err(|_| invalid("档案校验任务未完成"))?
}

fn invalid(message: &str) -> AppError {
    AppError::Internal(format!("身份档案校验失败：{message}").into())
}

fn io_error(error: std::io::Error) -> AppError {
    // Do not include image buffers, user descriptions or provider payloads.
    AppError::Internal(format!("身份档案文件操作失败：{error}").into())
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn valid_mime(value: &str) -> bool {
    value.len() <= 80
        && value.strip_prefix("image/").is_some_and(|subtype| {
            !subtype.is_empty()
                && subtype
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'+' | b'.'))
        })
}

fn valid_asset_fields(role: &str, ordinal: i16, original_type: &str, image_type: &str) -> bool {
    matches!(role, "source" | "portrait")
        && ordinal >= 0
        && valid_mime(original_type)
        && valid_mime(image_type)
}

fn build_manifest(
    companion_id: Uuid,
    identity: &Value,
    accessories: &Value,
    assets: &[ArchiveAsset],
) -> Result<Manifest> {
    if assets.is_empty() || assets.len() > 8 || !identity.is_object() || !accessories.is_array() {
        return Err(invalid("身份、配饰或参考图数量无效"));
    }
    let mut ids = HashSet::new();
    let mut positions = HashSet::new();
    let mut records = Vec::with_capacity(assets.len());
    for asset in assets {
        if !ids.insert(asset.id)
            || !positions.insert((asset.role.as_str(), asset.ordinal))
            || !valid_asset_fields(
                &asset.role,
                asset.ordinal,
                &asset.content_type,
                &asset.image_content_type,
            )
            || asset.original_bytes.is_empty()
            || asset.image_bytes.is_empty()
            || asset.original_bytes.len() as u64 > MAX_ASSET_BYTES
            || asset.image_bytes.len() as u64 > MAX_ASSET_BYTES
            || asset.sha256.len() != 32
            || Sha256::digest(&asset.original_bytes)[..] != asset.sha256[..]
        {
            return Err(invalid("图片元数据或原图 SHA-256 不匹配"));
        }
        records.push(AssetManifest {
            id: asset.id,
            role: asset.role.clone(),
            ordinal: asset.ordinal,
            content_type: asset.content_type.clone(),
            image_content_type: asset.image_content_type.clone(),
            original_file: format!("{}.original", asset.id),
            image_file: format!("{}.image", asset.id),
            sha256: hex::encode(&asset.sha256),
            image_sha256: hash(&asset.image_bytes),
            original_size: asset.original_bytes.len() as u64,
            image_size: asset.image_bytes.len() as u64,
        });
    }
    records.sort_by(|left, right| {
        (&left.role, left.ordinal, left.id).cmp(&(&right.role, right.ordinal, right.id))
    });
    Ok(Manifest {
        schema_version: 1,
        companion_id,
        identity: identity.clone(),
        accessories: accessories.clone(),
        assets: records,
    })
}

fn write_archive(
    directory: &Path,
    companion_id: Uuid,
    identity: &Value,
    accessories: &Value,
    assets: &[ArchiveAsset],
) -> Result<()> {
    // Validate all supplied originals before touching the archive volume.
    let manifest = build_manifest(companion_id, identity, accessories, assets)?;
    let encoded = serde_json::to_vec_pretty(&manifest)?;
    if encoded.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("身份说明过长"));
    }
    ensure_private_root(directory)?;
    let final_directory = directory.join(companion_id.to_string());
    if exists_no_follow(&final_directory)? {
        return compare_existing(&final_directory, companion_id, &manifest);
    }
    let staging = directory.join(format!(".tmp-{companion_id}-{}", Uuid::new_v4()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(&staging).map_err(io_error)?;
    let _cleanup = StagingDirectory(staging.clone());
    for asset in assets {
        write_new_file(
            &staging.join(format!("{}.original", asset.id)),
            &asset.original_bytes,
        )?;
        write_new_file(
            &staging.join(format!("{}.image", asset.id)),
            &asset.image_bytes,
        )?;
    }
    write_new_file(&staging.join(MANIFEST), &encoded)?;
    write_new_file(
        &staging.join(MANIFEST_HASH),
        format!("{}\n", hash(&encoded)).as_bytes(),
    )?;
    sync_directory(&staging)?;
    // Renaming a complete directory is atomic. POSIX never replaces a nonempty
    // destination directory, including one simultaneously published by a peer.
    // No target file is individually opened for writing or truncated.
    match fs::rename(&staging, &final_directory) {
        Ok(()) => {
            sync_directory(directory)?;
            compare_existing(&final_directory, companion_id, &manifest)
        }
        Err(error) => {
            if exists_no_follow(&final_directory)? {
                compare_existing(&final_directory, companion_id, &manifest)
            } else {
                Err(io_error(error))
            }
        }
    }
}

fn compare_existing(directory: &Path, companion_id: Uuid, expected: &Manifest) -> Result<()> {
    let existing = read_verified_archive(directory, companion_id)?;
    // The database asset UUID is an implementation detail.  A migration or a
    // recovery can recreate the row while restoring the exact same original
    // bytes.  The archive is the durable identity copy, so compare the
    // content and its role rather than requiring that transient row UUID to
    // be unchanged.  `read_verified_archive` has already checked each
    // archive filename, size and digest against its own manifest.
    if !equivalent_manifest(&existing, expected) {
        return Err(AppError::ArchiveConflict);
    }
    Ok(())
}

fn equivalent_manifest(left: &Manifest, right: &Manifest) -> bool {
    if left.schema_version != right.schema_version
        || left.companion_id != right.companion_id
        || left.identity != right.identity
        || left.accessories != right.accessories
        || left.assets.len() != right.assets.len()
    {
        return false;
    }
    let mut left_assets = left.assets.iter().collect::<Vec<_>>();
    let mut right_assets = right.assets.iter().collect::<Vec<_>>();
    left_assets.sort_by_key(|asset| (&asset.role, asset.ordinal));
    right_assets.sort_by_key(|asset| (&asset.role, asset.ordinal));
    left_assets.iter().zip(right_assets).all(|(left, right)| {
        left.role == right.role
            && left.ordinal == right.ordinal
            && left.content_type == right.content_type
            && left.image_content_type == right.image_content_type
            && left.sha256 == right.sha256
            && left.image_sha256 == right.image_sha256
            && left.original_size == right.original_size
            && left.image_size == right.image_size
    })
}

fn read_verified_archive(directory: &Path, companion_id: Uuid) -> Result<Manifest> {
    require_private_directory(directory)?;
    let encoded = read_private_file(&directory.join(MANIFEST), MAX_MANIFEST_BYTES)?;
    let checksum = read_private_file(&directory.join(MANIFEST_HASH), 65)?;
    if checksum != format!("{}\n", hash(&encoded)).as_bytes() {
        return Err(invalid("人物清单校验值不匹配"));
    }
    let manifest: Manifest = serde_json::from_slice(&encoded)?;
    if manifest.schema_version != 1
        || manifest.companion_id != companion_id
        || !manifest.identity.is_object()
        || !manifest.accessories.is_array()
        || manifest.assets.is_empty()
        || manifest.assets.len() > 8
    {
        return Err(invalid("人物清单结构或身份编号无效"));
    }
    let mut expected_files = HashSet::from([MANIFEST.to_string(), MANIFEST_HASH.to_string()]);
    let mut positions = HashSet::new();
    for asset in &manifest.assets {
        if asset.original_file != format!("{}.original", asset.id)
            || asset.image_file != format!("{}.image", asset.id)
            || !expected_files.insert(asset.original_file.clone())
            || !expected_files.insert(asset.image_file.clone())
            || !positions.insert((&asset.role, asset.ordinal))
            || !valid_asset_fields(
                &asset.role,
                asset.ordinal,
                &asset.content_type,
                &asset.image_content_type,
            )
            || !(1..=MAX_ASSET_BYTES).contains(&asset.original_size)
            || !(1..=MAX_ASSET_BYTES).contains(&asset.image_size)
        {
            return Err(invalid("图片清单结构无效"));
        }
        for (filename, expected_size, expected_hash) in [
            (&asset.original_file, asset.original_size, &asset.sha256),
            (&asset.image_file, asset.image_size, &asset.image_sha256),
        ] {
            let bytes = read_private_file(&directory.join(filename), expected_size)?;
            if bytes.len() as u64 != expected_size || hash(&bytes) != *expected_hash {
                return Err(invalid("原图或处理后图片缺失、大小改变或 SHA-256 不匹配"));
            }
        }
    }
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| expected_files.remove(name))
        {
            return Err(invalid("档案目录中存在未记录的文件"));
        }
    }
    if !expected_files.is_empty() {
        return Err(invalid("档案目录中缺少文件"));
    }
    Ok(manifest)
}

fn exists_no_follow(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn check_directory_chain(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(invalid("请配置专用的绝对档案目录"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component.as_os_str()),
            _ => return Err(invalid("档案路径包含不允许的路径片段")),
        }
        let metadata = fs::symlink_metadata(&current).map_err(io_error)?;
        if !metadata.file_type().is_dir() {
            return Err(invalid("档案目录不能包含符号链接或普通文件"));
        }
    }
    Ok(())
}

fn ensure_private_root(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(invalid("请配置专用的绝对档案目录"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Normal(_) => current.push(component.as_os_str()),
            _ => return Err(invalid("档案路径包含不允许的路径片段")),
        }
        if !exists_no_follow(&current)? {
            create_private_directory(&current)?;
        }
        let metadata = fs::symlink_metadata(&current).map_err(io_error)?;
        if !metadata.file_type().is_dir() {
            return Err(invalid("档案目录不能包含符号链接或普通文件"));
        }
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    require_private_directory(path)
}

fn create_private_directory(path: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            require_private_directory(path)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn require_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(invalid("档案路径不是普通目录"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o7777 != 0o700 {
        return Err(invalid("档案目录权限必须为 0700"));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

fn read_private_file(path: &Path, max_size: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.file_type().is_file() || metadata.len() > max_size {
        return Err(invalid("档案文件类型或大小无效"));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o7777 != 0o600 {
        return Err(invalid("档案文件权限必须为 0600"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    // Linux's O_NOFOLLOW prevents a final-component symlink swap between the
    // metadata check and open. Ancestors are private UUID directories checked
    // above; no uploaded filename ever contributes a path component.
    #[cfg(target_os = "linux")]
    options.custom_flags(0o400000);
    let file = options.open(path).map_err(io_error)?;
    #[cfg(unix)]
    {
        let opened = file.metadata().map_err(io_error)?;
        if opened.ino() != metadata.ino() || opened.dev() != metadata.dev() {
            return Err(invalid("档案文件在校验时发生变化"));
        }
    }
    let mut bytes = Vec::new();
    file.take(max_size + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > max_size {
        return Err(invalid("档案文件超出记录大小"));
    }
    Ok(bytes)
}

fn sync_directory(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    options.custom_flags(0o600000); // O_DIRECTORY | O_NOFOLLOW.
    options
        .open(path)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)
}

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("along-archive-test-{}", Uuid::new_v4())))
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn asset() -> ArchiveAsset {
        let bytes = b"original image bytes".to_vec();
        ArchiveAsset {
            id: Uuid::new_v4(),
            role: "portrait".into(),
            ordinal: 0,
            content_type: "image/png".into(),
            image_content_type: "image/jpeg".into(),
            sha256: Sha256::digest(&bytes).to_vec(),
            original_bytes: bytes,
            image_bytes: b"normalized image bytes".to_vec(),
        }
    }
    #[tokio::test]
    async fn archive_is_idempotent_and_verified_without_rewriting() {
        let root = TestDirectory::new();
        let companion = Uuid::new_v4();
        let assets = [asset()];
        let identity = json!({"name_seed":"同行", "gender":"male"});
        write_identity_archive(&root.0, companion, &identity, &json!([]), &assets)
            .await
            .unwrap();
        let before = fs::read(root.0.join(companion.to_string()).join(MANIFEST)).unwrap();
        write_identity_archive(&root.0, companion, &identity, &json!([]), &assets)
            .await
            .unwrap();
        verify_identity_archive(&root.0, companion).await.unwrap();
        assert_eq!(
            before,
            fs::read(root.0.join(companion.to_string()).join(MANIFEST)).unwrap()
        );
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&root.0).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for entry in fs::read_dir(root.0.join(companion.to_string())).unwrap() {
                assert_eq!(
                    entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
    #[tokio::test]
    async fn archive_rejects_changed_identity_or_supplied_original_hash() {
        let root = TestDirectory::new();
        let companion = Uuid::new_v4();
        let mut assets = [asset()];
        write_identity_archive(
            &root.0,
            companion,
            &json!({"name":"first"}),
            &json!([]),
            &assets,
        )
        .await
        .unwrap();
        assert!(matches!(
            write_identity_archive(
                &root.0,
                companion,
                &json!({"name":"second"}),
                &json!([]),
                &assets
            )
            .await,
            Err(AppError::ArchiveConflict)
        ));
        // Asset UUIDs belong to database rows and may change during a
        // restore.  Identical original/reference bytes still describe the
        // same permanent identity and must not trigger a rewrite.
        assets[0].id = Uuid::new_v4();
        write_identity_archive(
            &root.0,
            companion,
            &json!({"name":"first"}),
            &json!([]),
            &assets,
        )
        .await
        .unwrap();
        assets[0].original_bytes.push(0);
        assert!(
            write_identity_archive(&root.0, Uuid::new_v4(), &json!({}), &json!([]), &assets)
                .await
                .is_err()
        );
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
        verify_identity_archive(&root.0, companion).await.unwrap();
    }
    #[tokio::test]
    async fn archive_detects_image_and_manifest_corruption() {
        let root = TestDirectory::new();
        let companion = Uuid::new_v4();
        let assets = [asset()];
        write_identity_archive(&root.0, companion, &json!({}), &json!([]), &assets)
            .await
            .unwrap();
        let directory = root.0.join(companion.to_string());
        let image_path = directory.join(format!("{}.image", assets[0].id));
        fs::write(&image_path, b"corrupted").unwrap();
        assert!(verify_identity_archive(&root.0, companion).await.is_err());
        fs::write(&image_path, &assets[0].image_bytes).unwrap();
        verify_identity_archive(&root.0, companion).await.unwrap();
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(directory.join(MANIFEST)).unwrap()).unwrap();
        manifest["identity"] = json!({"changed":true});
        fs::write(
            directory.join(MANIFEST),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(verify_identity_archive(&root.0, companion).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn archive_rejects_symlink_roots_and_files() {
        use std::os::unix::fs::symlink;
        let root = TestDirectory::new();
        let companion = Uuid::new_v4();
        let assets = [asset()];
        write_identity_archive(&root.0, companion, &json!({}), &json!([]), &assets)
            .await
            .unwrap();
        let alias = TestDirectory::new();
        symlink(&root.0, &alias.0).unwrap();
        assert!(verify_identity_archive(&alias.0, companion).await.is_err());
        assert!(
            write_identity_archive(&alias.0, companion, &json!({}), &json!([]), &assets)
                .await
                .is_err()
        );
        fs::remove_file(&alias.0).unwrap();
        let directory = root.0.join(companion.to_string());
        let image_path = directory.join(format!("{}.image", assets[0].id));
        fs::remove_file(&image_path).unwrap();
        symlink(
            directory.join(format!("{}.original", assets[0].id)),
            &image_path,
        )
        .unwrap();
        assert!(verify_identity_archive(&root.0, companion).await.is_err());
    }
    #[tokio::test]
    async fn concurrent_publication_cannot_replace_a_different_identity() {
        let root = TestDirectory::new();
        let companion = Uuid::new_v4();
        let assets = [asset()];
        let first = json!({"name":"first"});
        let second = json!({"name":"second"});
        let accessories = json!([]);
        let (one, two) = tokio::join!(
            write_identity_archive(&root.0, companion, &first, &accessories, &assets),
            write_identity_archive(&root.0, companion, &second, &accessories, &assets),
        );
        assert_ne!(one.is_ok(), two.is_ok());
        verify_identity_archive(&root.0, companion).await.unwrap();
    }
}

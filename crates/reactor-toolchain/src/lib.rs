use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use fs2::FileExt;
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedToolsManifest {
    pub schema_version: u32,
    pub tools: Vec<ToolSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub id: String,
    pub version: String,
    pub install_dir: String,
    pub executable_names: Vec<String>,
    pub assets: BTreeMap<String, AssetSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetSpec {
    pub url: String,
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: Option<u64>,
    pub archive: ArchiveKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveKind {
    Zip,
    TarGz,
    Binary,
}

#[derive(Debug, Clone, Default)]
pub struct SetupOptions {
    pub offline: bool,
    pub proxy: Option<String>,
    pub maestro_override: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledTool {
    pub id: String,
    pub version: String,
    pub source: String,
    pub executable: String,
    pub archive_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledManifest {
    pub schema_version: u32,
    pub installed_at: DateTime<Utc>,
    pub host: String,
    pub tools: Vec<InstalledTool>,
}

#[derive(Debug, Clone)]
pub struct ToolLayout {
    pub root: PathBuf,
    pub downloads: PathBuf,
    pub manifest: PathBuf,
}

#[derive(Debug, Error)]
pub enum ToolchainError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error("unsupported Reactor host: {0}")]
    UnsupportedHost(String),
    #[error("tool {tool} has no asset for host {host}")]
    MissingAsset { tool: String, host: String },
    #[error("offline cache miss for {0}")]
    OfflineCacheMiss(String),
    #[error("download failed ({status}) for {url}")]
    DownloadFailed { status: StatusCode, url: String },
    #[error("checksum mismatch for {path}: expected {expected}, received {actual}")]
    ChecksumMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("download size mismatch for {path}: expected {expected}, received {actual}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
    #[error("archive contains an unsafe path: {0}")]
    UnsafeArchivePath(String),
    #[error("managed executable was not found for {0}")]
    ExecutableMissing(String),
    #[error("Maestro override does not exist or is not a file: {0}")]
    InvalidOverride(String),
}

#[must_use]
pub fn layout(workspace: &Path) -> ToolLayout {
    let runtime_root = workspace.join(".reactor");
    ToolLayout {
        root: runtime_root.join("tools"),
        downloads: runtime_root.join("downloads"),
        manifest: runtime_root.join("tools/manifest-v2.json"),
    }
}

/// Returns Reactor's canonical host identifier used by the pinned manifest.
///
/// # Errors
///
/// Returns an error for hosts that Reactor does not currently package.
pub fn host_id() -> Result<String, ToolchainError> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => return Err(ToolchainError::UnsupportedHost(other.to_owned())),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => return Err(ToolchainError::UnsupportedHost(format!("{os}-{other}"))),
    };
    Ok(format!("{os}-{arch}"))
}

/// Installs or reuses all tools for the current host from a pinned manifest.
///
/// The cache is checksum-verified in both online and offline mode. A process lock prevents two
/// Reactor instances from replacing the same installation concurrently.
///
/// # Errors
///
/// Returns an error for a cache miss, network failure, checksum mismatch, unsafe archive, or
/// missing executable.
pub async fn setup(
    workspace: &Path,
    manifest: &ManagedToolsManifest,
    options: &SetupOptions,
) -> Result<InstalledManifest, ToolchainError> {
    let host = host_id()?;
    let paths = layout(workspace);
    std::fs::create_dir_all(&paths.root)?;
    std::fs::create_dir_all(&paths.downloads)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(paths.root.join("setup.lock"))?;
    lock.lock_exclusive()?;

    let client = build_client(options.proxy.as_deref())?;
    let previous = std::fs::read(&paths.manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<InstalledManifest>(&bytes).ok());
    let mut installed = Vec::with_capacity(manifest.tools.len());
    for tool in &manifest.tools {
        if tool.id == "maestro"
            && let Some(override_path) = options.maestro_override.as_deref()
        {
            if !override_path.is_file() {
                return Err(ToolchainError::InvalidOverride(
                    override_path.display().to_string(),
                ));
            }
            installed.push(InstalledTool {
                id: tool.id.clone(),
                version: tool.version.clone(),
                source: "local_override".to_owned(),
                executable: override_path.display().to_string(),
                archive_sha256: None,
            });
            continue;
        }
        let asset = tool
            .assets
            .get(&host)
            .or_else(|| tool.assets.get("all"))
            .ok_or_else(|| ToolchainError::MissingAsset {
                tool: tool.id.clone(),
                host: host.clone(),
            })?;
        if let Some(existing) = previous.as_ref().and_then(|installed| {
            installed.tools.iter().find(|candidate| {
                candidate.id == tool.id
                    && candidate.version == tool.version
                    && candidate.archive_sha256.as_deref() == Some(asset.sha256.as_str())
                    && Path::new(&candidate.executable).is_file()
            })
        }) {
            installed.push(existing.clone());
            continue;
        }
        let archive = paths.downloads.join(&asset.file_name);
        ensure_cached(&client, asset, &archive, options.offline).await?;
        let target = paths.root.join(&tool.install_dir);
        install_archive(&archive, &target, asset.archive)?;
        let executable = find_executable(&target, &tool.executable_names)
            .ok_or_else(|| ToolchainError::ExecutableMissing(tool.id.clone()))?;
        make_executable(&executable)?;
        installed.push(InstalledTool {
            id: tool.id.clone(),
            version: tool.version.clone(),
            source: "managed_release".to_owned(),
            executable: executable.display().to_string(),
            archive_sha256: Some(asset.sha256.clone()),
        });
    }
    let result = InstalledManifest {
        schema_version: 2,
        installed_at: Utc::now(),
        host,
        tools: installed,
    };
    write_json_atomic(&paths.manifest, &result)?;
    FileExt::unlock(&lock)?;
    Ok(result)
}

fn build_client(proxy: Option<&str>) -> Result<Client, ToolchainError> {
    let mut builder = Client::builder().redirect(reqwest::redirect::Policy::limited(10));
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    Ok(builder.build()?)
}

async fn ensure_cached(
    client: &Client,
    asset: &AssetSpec,
    destination: &Path,
    offline: bool,
) -> Result<(), ToolchainError> {
    if destination.is_file() {
        match verify_file(destination, asset) {
            Ok(()) => return Ok(()),
            Err(error) if offline => return Err(error),
            Err(_) => std::fs::remove_file(destination)?,
        }
    }
    if offline {
        return Err(ToolchainError::OfflineCacheMiss(
            destination.display().to_string(),
        ));
    }
    let partial = partial_path(destination);
    let existing = std::fs::metadata(&partial).map_or(0, |metadata| metadata.len());
    let mut request = client.get(&asset.url);
    if existing > 0 {
        request = request.header(header::RANGE, format!("bytes={existing}-"));
    }
    let response = request.send().await?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && existing > 0 {
        verify_file(&partial, asset)?;
        std::fs::rename(partial, destination)?;
        return Ok(());
    }
    if !response.status().is_success() {
        return Err(ToolchainError::DownloadFailed {
            status: response.status(),
            url: asset.url.clone(),
        });
    }
    let append = existing > 0 && response.status() == StatusCode::PARTIAL_CONTENT;
    if append {
        let expected_prefix = format!("bytes {existing}-");
        let matches_offset = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with(&expected_prefix));
        if !matches_offset {
            return Err(ToolchainError::DownloadFailed {
                status: response.status(),
                url: asset.url.clone(),
            });
        }
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&partial)
        .await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk?).await?;
    }
    file.flush().await?;
    drop(file);
    verify_file(&partial, asset)?;
    std::fs::rename(partial, destination)?;
    Ok(())
}

fn partial_path(destination: &Path) -> PathBuf {
    destination.with_extension(format!(
        "{}part",
        destination
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .map_or(String::new(), |extension| format!("{extension}."))
    ))
}

fn verify_file(path: &Path, asset: &AssetSpec) -> Result<(), ToolchainError> {
    let size = std::fs::metadata(path)?.len();
    if let Some(expected) = asset.size_bytes
        && size != expected
    {
        return Err(ToolchainError::SizeMismatch {
            path: path.display().to_string(),
            expected,
            actual: size,
        });
    }
    let actual = hash_file(path)?;
    if actual.eq_ignore_ascii_case(&asset.sha256) {
        Ok(())
    } else {
        Err(ToolchainError::ChecksumMismatch {
            path: path.display().to_string(),
            expected: asset.sha256.clone(),
            actual,
        })
    }
}

fn hash_file(path: &Path) -> Result<String, ToolchainError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn install_archive(archive: &Path, target: &Path, kind: ArchiveKind) -> Result<(), ToolchainError> {
    let staging = target.with_extension("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    match kind {
        ArchiveKind::Zip => extract_zip(archive, &staging)?,
        ArchiveKind::TarGz => {
            let decoder = GzDecoder::new(File::open(archive)?);
            let mut tar = tar::Archive::new(decoder);
            for entry in tar.entries()? {
                let mut entry = entry?;
                let relative = entry.path()?.into_owned();
                ensure_relative_path(&relative)?;
                if !entry.unpack_in(&staging)? {
                    return Err(ToolchainError::UnsafeArchivePath(
                        relative.display().to_string(),
                    ));
                }
            }
        }
        ArchiveKind::Binary => {
            let name = archive
                .file_name()
                .ok_or_else(|| ToolchainError::UnsafeArchivePath(archive.display().to_string()))?;
            std::fs::copy(archive, staging.join(name))?;
        }
    }
    replace_directory(&staging, target)?;
    Ok(())
}

fn replace_directory(staging: &Path, target: &Path) -> Result<(), ToolchainError> {
    let backup = target.with_extension("previous");
    if backup.exists() {
        std::fs::remove_dir_all(&backup)?;
    }
    if target.exists() {
        std::fs::rename(target, &backup)?;
    }
    if let Err(error) = std::fs::rename(staging, target) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, target);
        }
        return Err(error.into());
    }
    if backup.exists() {
        std::fs::remove_dir_all(backup)?;
    }
    Ok(())
}

fn extract_zip(archive: &Path, destination: &Path) -> Result<(), ToolchainError> {
    let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| ToolchainError::UnsafeArchivePath(entry.name().to_owned()))?;
        ensure_relative_path(&relative)?;
        let output = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&output)?;
        std::io::copy(&mut entry, &mut file)?;
        if let Some(mode) = entry.unix_mode() {
            set_mode(&output, mode)?;
        }
    }
    Ok(())
}

fn ensure_relative_path(path: &Path) -> Result<(), ToolchainError> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        Ok(())
    } else {
        Err(ToolchainError::UnsafeArchivePath(
            path.display().to_string(),
        ))
    }
}

fn find_executable(root: &Path, names: &[String]) -> Option<PathBuf> {
    WalkDir::new(root)
        .max_depth(10)
        .into_iter()
        .filter_map(Result::ok)
        .find(|entry| {
            entry.file_type().is_file()
                && names.iter().any(|name| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(name)
                })
        })
        .map(walkdir::DirEntry::into_path)
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), ToolchainError> {
    let parent = path
        .parent()
        .ok_or_else(|| ToolchainError::UnsafeArchivePath(path.display().to_string()))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), ToolchainError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), ToolchainError> {
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), ToolchainError> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), ToolchainError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_manifest(url: String, file_name: &str, bytes: &[u8]) -> ManagedToolsManifest {
        ManagedToolsManifest {
            schema_version: 1,
            tools: vec![ToolSpec {
                id: "fixture".to_owned(),
                version: "1".to_owned(),
                install_dir: "fixture-1".to_owned(),
                executable_names: vec![file_name.to_owned()],
                assets: BTreeMap::from([(
                    "all".to_owned(),
                    AssetSpec {
                        url,
                        file_name: file_name.to_owned(),
                        sha256: hex::encode(Sha256::digest(bytes)),
                        size_bytes: Some(bytes.len() as u64),
                        archive: ArchiveKind::Binary,
                    },
                )]),
            }],
        }
    }

    #[test]
    fn rejects_parent_archive_paths() {
        assert!(ensure_relative_path(Path::new("../escape")).is_err());
        assert!(ensure_relative_path(Path::new("bin/tool")).is_ok());
    }

    #[tokio::test]
    async fn offline_mode_reuses_verified_binary_cache() {
        let workspace = std::env::temp_dir().join(format!(
            "reactor-toolchain-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let paths = layout(&workspace);
        std::fs::create_dir_all(&paths.downloads).unwrap();
        let bytes = b"managed executable";
        let cache = paths.downloads.join("tool.bin");
        std::fs::write(&cache, bytes).unwrap();
        let manifest = fixture_manifest(
            "https://invalid.example/tool.bin".to_owned(),
            "tool.bin",
            bytes,
        );
        let installed = setup(
            &workspace,
            &manifest,
            &SetupOptions {
                offline: true,
                ..SetupOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(installed.tools[0].source, "managed_release");
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn resumes_partial_download_with_range_request() {
        use std::io::{Read as _, Write as _};

        let bytes = b"complete managed tool payload";
        let split = 9;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = vec![0_u8; 4096];
            let count = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]);
            assert!(request.contains("range: bytes=9-") || request.contains("Range: bytes=9-"));
            let body = &bytes[split..];
            let response = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {split}-{}/{}\r\nConnection: close\r\n\r\n",
                body.len(),
                bytes.len() - 1,
                bytes.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
            socket.write_all(body).unwrap();
        });
        let workspace = std::env::temp_dir().join(format!(
            "reactor-resume-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let paths = layout(&workspace);
        std::fs::create_dir_all(&paths.downloads).unwrap();
        let destination = paths.downloads.join("resume.bin");
        std::fs::write(partial_path(&destination), &bytes[..split]).unwrap();
        let manifest =
            fixture_manifest(format!("http://{address}/resume.bin"), "resume.bin", bytes);

        setup(&workspace, &manifest, &SetupOptions::default())
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), bytes);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn maestro_override_never_downloads_managed_asset() {
        let workspace = std::env::temp_dir().join(format!(
            "reactor-override-{}",
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let override_path = workspace.join("maestro-local");
        std::fs::write(&override_path, b"local fork").unwrap();
        let mut manifest = fixture_manifest(
            "https://invalid.example/maestro.zip".to_owned(),
            "maestro",
            b"never downloaded",
        );
        manifest.tools[0].id = "maestro".to_owned();

        let installed = setup(
            &workspace,
            &manifest,
            &SetupOptions {
                offline: true,
                maestro_override: Some(override_path.clone()),
                proxy: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(installed.tools[0].source, "local_override");
        assert_eq!(
            installed.tools[0].executable,
            override_path.display().to_string()
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }
}

use std::{
    cmp::Ordering,
    fs::File,
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use uuid::Uuid;

use super::{UpdateArtifact, UpdateManifestV1, validate_signed_update_manifest};

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_UPDATE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpdatePhase {
    Staged,
    Activating,
    Probing,
    Healthy,
    RolledBack,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateTransaction {
    pub transaction_id: String,
    pub version: String,
    pub phase: UpdatePhase,
    pub workspace: PathBuf,
    pub current_install: PathBuf,
    pub ready_install: PathBuf,
    pub backup_install: PathBuf,
    pub database_path: PathBuf,
    pub database_backup: PathBuf,
    pub created_at: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StagedUpdate {
    pub channel: String,
    pub version: String,
    pub transaction_path: String,
    pub artifact_bytes: u64,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateTransactionView {
    pub version: String,
    pub phase: UpdatePhase,
    pub created_at: DateTime<Utc>,
    pub error: Option<String>,
}

pub(crate) fn latest_transaction(workspace: &Path) -> Option<UpdateTransactionView> {
    let root = workspace.join(".reactor/updates/transactions");
    let mut transactions = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| read_transaction(&entry.path().join("transaction.json")).ok())
        .collect::<Vec<_>>();
    transactions.sort_by_key(|transaction| transaction.created_at);
    transactions.pop().map(|transaction| UpdateTransactionView {
        version: transaction.version,
        phase: transaction.phase,
        created_at: transaction.created_at,
        error: transaction.error,
    })
}

pub(crate) async fn fetch_and_stage(
    workspace: &Path,
    endpoint: &str,
    expected_channel: &str,
    public_key: &str,
    supported_database_schema: i64,
    current_install: &Path,
) -> Result<StagedUpdate, String> {
    if !endpoint.starts_with("https://") {
        return Err("更新 manifest 必须使用 HTTPS".to_owned());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(UPDATE_HTTP_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let manifest_bytes = download_bounded(&client, endpoint, MAX_MANIFEST_BYTES).await?;
    let manifest: UpdateManifestV1 =
        serde_json::from_slice(&manifest_bytes).map_err(|error| error.to_string())?;
    validate_signed_update_manifest(&manifest, public_key, supported_database_schema)?;
    if manifest.channel != expected_channel {
        return Err(format!(
            "{expected_channel} 更新端点返回了 {} manifest",
            manifest.channel
        ));
    }
    if compare_versions(&manifest.version, env!("CARGO_PKG_VERSION"))? != Ordering::Greater {
        return Err(format!(
            "当前版本 {} 已不低于更新版本 {}",
            env!("CARGO_PKG_VERSION"),
            manifest.version
        ));
    }
    if compare_versions(
        env!("CARGO_PKG_VERSION"),
        &manifest.compatibility.minimum_app_version,
    )? == Ordering::Less
    {
        return Err("当前版本低于该更新允许的最小升级版本".to_owned());
    }
    let artifact = select_host_artifact(&manifest)?;
    if artifact.size > MAX_UPDATE_BYTES {
        return Err("更新包超过 1 GiB 安全上限".to_owned());
    }
    let download_root = workspace.join(".reactor/updates/downloads");
    std::fs::create_dir_all(&download_root).map_err(|error| error.to_string())?;
    let archive_path = download_root.join(format!("{}.archive", Uuid::new_v4()));
    if let Err(error) =
        download_file_bounded(&client, &artifact.url, artifact.size, &archive_path).await
    {
        let _ = std::fs::remove_file(&archive_path);
        return Err(error);
    }
    verify_archive(&archive_path, artifact)?;
    let staged = prepare_transaction(
        workspace,
        current_install,
        &manifest,
        artifact,
        &archive_path,
    );
    let _ = std::fs::remove_file(&archive_path);
    staged
}

fn verify_archive(path: &Path, artifact: &UpdateArtifact) -> Result<(), String> {
    let actual_size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if actual_size != artifact.size {
        return Err(format!(
            "更新包大小不匹配：期望 {}，实际 {}",
            artifact.size, actual_size
        ));
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let actual_hash = hex::encode(hash.finalize());
    if !actual_hash.eq_ignore_ascii_case(&artifact.sha256) {
        return Err("更新包 SHA-256 校验失败".to_owned());
    }
    Ok(())
}

async fn download_file_bounded(
    client: &reqwest::Client,
    url: &str,
    maximum: u64,
    destination: &Path,
) -> Result<(), String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.url().scheme() != "https" || !response.status().is_success() {
        return Err(format!("更新包下载失败：HTTP {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum)
    {
        return Err("更新包下载超过声明大小".to_owned());
    }
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|error| error.to_string())?;
    let mut received = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        received = received.saturating_add(chunk.len() as u64);
        if received > maximum {
            let _ = tokio::fs::remove_file(destination).await;
            return Err("更新包下载超过声明大小".to_owned());
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    file.flush().await.map_err(|error| error.to_string())?;
    Ok(())
}

async fn download_bounded(
    client: &reqwest::Client,
    url: &str,
    maximum: u64,
) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.url().scheme() != "https" {
        return Err("更新下载重定向到了非 HTTPS 地址".to_owned());
    }
    if !response.status().is_success() {
        return Err(format!("更新下载失败：HTTP {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum)
    {
        return Err("更新下载超过声明的大小上限".to_owned());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if bytes.len() as u64 + chunk.len() as u64 > maximum {
            return Err("更新下载超过声明的大小上限".to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "arm" => "armv7",
        other => other,
    }
}

fn select_host_artifact(manifest: &UpdateManifestV1) -> Result<&UpdateArtifact, String> {
    manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.platform == host_platform() && artifact.arch == host_arch())
        .ok_or_else(|| {
            format!(
                "更新没有适用于 {} / {} 的安装包",
                host_platform(),
                host_arch()
            )
        })
}

fn parse_version(value: &str) -> Result<Vec<u64>, String> {
    let core = value
        .trim()
        .strip_prefix('v')
        .unwrap_or(value.trim())
        .split_once(['-', '+'])
        .map_or_else(|| value.trim().trim_start_matches('v'), |(core, _)| core);
    let parts = core
        .split('.')
        .map(|part| {
            part.parse::<u64>()
                .map_err(|_| format!("无效版本号：{value}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parts.is_empty() {
        return Err(format!("无效版本号：{value}"));
    }
    Ok(parts)
}

fn compare_versions(left: &str, right: &str) -> Result<Ordering, String> {
    let mut left = parse_version(left)?;
    let mut right = parse_version(right)?;
    let length = left.len().max(right.len());
    left.resize(length, 0);
    right.resize(length, 0);
    Ok(left.cmp(&right))
}

fn prepare_transaction(
    workspace: &Path,
    current_install: &Path,
    manifest: &UpdateManifestV1,
    artifact: &UpdateArtifact,
    archive: &Path,
) -> Result<StagedUpdate, String> {
    validate_candidate_install(current_install)?;
    std::fs::create_dir_all(workspace).map_err(|error| error.to_string())?;
    let install_parent = current_install
        .parent()
        .ok_or_else(|| "无法定位当前 Reactor 安装目录".to_owned())?;
    let required_bytes = artifact
        .size
        .saturating_mul(3)
        .saturating_add(64 * 1024 * 1024);
    for location in [workspace, install_parent] {
        let available = fs2::available_space(location).map_err(|error| error.to_string())?;
        if available < required_bytes {
            return Err(format!(
                "更新空间不足：至少需要 {} MiB 可用空间",
                required_bytes / 1024 / 1024
            ));
        }
    }
    let transaction_id = Uuid::new_v4().to_string();
    let transaction_root = workspace
        .join(".reactor/updates/transactions")
        .join(&transaction_id);
    let extracted = transaction_root.join("extracted");
    std::fs::create_dir_all(&extracted).map_err(|error| error.to_string())?;
    extract_archive(archive, &artifact.url, &extracted)?;
    let candidate = find_candidate_install(&extracted)?;
    validate_candidate_install(&candidate)?;

    let ready_install = install_parent.join(format!(".Reactor-update-{transaction_id}.app"));
    let backup_install = install_parent.join(format!(".Reactor-backup-{transaction_id}.app"));
    if ready_install.exists() {
        std::fs::remove_dir_all(&ready_install).map_err(|error| error.to_string())?;
    }
    copy_directory(&candidate, &ready_install)?;
    validate_candidate_install(&ready_install)?;

    let database_path = workspace.join(".reactor/runtime/reactor.sqlite3");
    let database_backup = transaction_root.join("database-backup");
    snapshot_database(&database_path, &database_backup)?;
    let mut transaction = UpdateTransaction {
        transaction_id,
        version: manifest.version.clone(),
        phase: UpdatePhase::Staged,
        workspace: workspace.to_path_buf(),
        current_install: current_install.to_path_buf(),
        ready_install,
        backup_install,
        database_path,
        database_backup,
        created_at: Utc::now(),
        error: None,
    };
    let transaction_path = transaction_root.join("transaction.json");
    persist_transaction(&transaction_path, &transaction)?;
    transaction.error = None;
    Ok(StagedUpdate {
        channel: manifest.channel.clone(),
        version: manifest.version.clone(),
        transaction_path: transaction_path.display().to_string(),
        artifact_bytes: artifact.size,
        restart_required: true,
    })
}

fn extract_archive(archive_file: &Path, url: &str, destination: &Path) -> Result<(), String> {
    let clean_url = url.split(['?', '#']).next().unwrap_or(url);
    let archive_path = Path::new(clean_url);
    let extension = archive_path.extension().and_then(std::ffi::OsStr::to_str);
    let is_tgz = extension.is_some_and(|value| value.eq_ignore_ascii_case("tgz"));
    let is_tar_gz = extension.is_some_and(|value| value.eq_ignore_ascii_case("gz"))
        && archive_path
            .file_stem()
            .map(Path::new)
            .and_then(Path::extension)
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("tar"));
    if is_tar_gz || is_tgz {
        let decoder = GzDecoder::new(File::open(archive_file).map_err(|error| error.to_string())?);
        let mut archive = tar::Archive::new(decoder);
        let mut extracted_bytes = 0_u64;
        for (index, entry) in archive
            .entries()
            .map_err(|error| error.to_string())?
            .enumerate()
        {
            if index >= MAX_ARCHIVE_ENTRIES {
                return Err("更新包文件数量超过安全上限".to_owned());
            }
            let mut entry = entry.map_err(|error| error.to_string())?;
            let relative = entry
                .path()
                .map_err(|error| error.to_string())?
                .into_owned();
            ensure_relative(&relative)?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                return Err("更新包包含不允许的链接或特殊文件".to_owned());
            }
            extracted_bytes = extracted_bytes.saturating_add(entry.size());
            if extracted_bytes > MAX_EXTRACTED_BYTES {
                return Err("更新包解压后超过 2 GiB 安全上限".to_owned());
            }
            if !entry
                .unpack_in(destination)
                .map_err(|error| error.to_string())?
            {
                return Err("更新包包含越界路径".to_owned());
            }
        }
        return Ok(());
    }
    if extension.is_some_and(|value| value.eq_ignore_ascii_case("zip")) {
        let reader = File::open(archive_file).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(reader).map_err(|error| error.to_string())?;
        if archive.len() > MAX_ARCHIVE_ENTRIES {
            return Err("更新包文件数量超过安全上限".to_owned());
        }
        let mut extracted_bytes = 0_u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            extracted_bytes = extracted_bytes.saturating_add(entry.size());
            if extracted_bytes > MAX_EXTRACTED_BYTES {
                return Err("更新包解压后超过 2 GiB 安全上限".to_owned());
            }
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| "更新包包含越界路径".to_owned())?;
            ensure_relative(&relative)?;
            let output = destination.join(relative);
            if entry.is_dir() {
                std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
                continue;
            }
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut file = File::create(&output).map_err(|error| error.to_string())?;
            std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
            set_mode(&output, entry.unix_mode())?;
        }
        return Ok(());
    }
    Err("更新包必须是 .tar.gz、.tgz 或 .zip".to_owned())
}

fn ensure_relative(path: &Path) -> Result<(), String> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        Ok(())
    } else {
        Err("更新包包含不安全路径".to_owned())
    }
}

fn find_candidate_install(root: &Path) -> Result<PathBuf, String> {
    let direct = root.join("Reactor.app");
    if direct.is_dir() {
        return Ok(direct);
    }
    let mut candidates = std::fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        Ok(candidates.remove(0))
    } else {
        Err("更新包必须只包含一个 Reactor.app".to_owned())
    }
}

fn candidate_executable(install: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        install.join("Contents/MacOS/reactor-desktop")
    }
    #[cfg(target_os = "windows")]
    {
        install.join("reactor-desktop.exe")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        install.join("reactor-desktop")
    }
}

fn validate_candidate_install(install: &Path) -> Result<(), String> {
    let executable = candidate_executable(install);
    if !executable.is_file() {
        return Err(format!("候选安装缺少可执行文件：{}", executable.display()));
    }
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&source_path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("候选安装包含不允许的符号链接".to_owned());
        }
        let target_path = destination.join(entry.file_name());
        if metadata.is_dir() {
            copy_directory(&source_path, &target_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &target_path).map_err(|error| error.to_string())?;
            std::fs::set_permissions(&target_path, metadata.permissions())
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn snapshot_database(database: &Path, backup: &Path) -> Result<(), String> {
    std::fs::create_dir_all(backup).map_err(|error| error.to_string())?;
    for suffix in ["", "-wal", "-shm"] {
        let source = PathBuf::from(format!("{}{suffix}", database.display()));
        if source.is_file() {
            std::fs::copy(&source, backup.join(format!("database{suffix}")))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn restore_database(database: &Path, backup: &Path) -> Result<(), String> {
    if let Some(parent) = database.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    for suffix in ["", "-wal", "-shm"] {
        let target = PathBuf::from(format!("{}{suffix}", database.display()));
        if target.exists() {
            std::fs::remove_file(&target).map_err(|error| error.to_string())?;
        }
        let source = backup.join(format!("database{suffix}"));
        if source.is_file() {
            std::fs::copy(source, target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn persist_transaction(path: &Path, transaction: &UpdateTransaction) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(transaction).map_err(|error| error.to_string())?;
    let mut file = File::create(&temporary).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
}

pub(crate) fn spawn_install_helper(transaction_path: &Path) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Command::new(executable)
        .arg("--reactor-update-helper")
        .arg(transaction_path)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(crate) fn run_helper(transaction_path: &Path, parent_pid: u32) -> Result<(), String> {
    wait_for_parent_exit(parent_pid)?;
    let mut transaction = read_transaction(transaction_path)?;
    validate_transaction_scope(transaction_path, &transaction)?;
    if transaction.phase != UpdatePhase::Staged {
        return Err("更新事务不处于可安装状态".to_owned());
    }
    transaction.phase = UpdatePhase::Activating;
    persist_transaction(transaction_path, &transaction)?;
    if let Err(error) = activate_install(&transaction) {
        transaction.phase = UpdatePhase::Quarantined;
        transaction.error = Some(error.clone());
        persist_transaction(transaction_path, &transaction)?;
        let _ = Command::new(candidate_executable(&transaction.current_install)).spawn();
        return Err(error);
    }
    transaction.phase = UpdatePhase::Probing;
    persist_transaction(transaction_path, &transaction)?;

    let probe = Command::new(candidate_executable(&transaction.current_install))
        .arg("--reactor-update-health-probe")
        .arg(&transaction.workspace)
        .arg(&transaction.version)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if probe.is_ok_and(|status| status.success()) {
        transaction.phase = UpdatePhase::Healthy;
        transaction.error = None;
        let persist_result = persist_transaction(transaction_path, &transaction);
        if transaction.backup_install.exists() {
            let _ = std::fs::remove_dir_all(&transaction.backup_install);
        }
        let launch_result = launch_current(&transaction);
        persist_result?;
        launch_result?;
    } else {
        let rollback_result = rollback_install(&transaction).and_then(|()| {
            restore_database(&transaction.database_path, &transaction.database_backup)
        });
        if let Err(error) = &rollback_result {
            transaction.phase = UpdatePhase::Quarantined;
            transaction.error = Some(format!("候选版本健康检查失败，自动回滚也失败：{error}"));
        } else {
            transaction.phase = UpdatePhase::RolledBack;
            transaction.error = Some("候选版本健康检查失败，已自动恢复上一版本".to_owned());
        }
        let persist_result = persist_transaction(transaction_path, &transaction);
        let launch_result = launch_current(&transaction);
        rollback_result?;
        persist_result?;
        launch_result?;
    }
    Ok(())
}

fn launch_current(transaction: &UpdateTransaction) -> Result<(), String> {
    Command::new(candidate_executable(&transaction.current_install))
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn activate_install(transaction: &UpdateTransaction) -> Result<(), String> {
    validate_candidate_install(&transaction.ready_install)?;
    if transaction.backup_install.exists() {
        std::fs::remove_dir_all(&transaction.backup_install).map_err(|error| error.to_string())?;
    }
    std::fs::rename(&transaction.current_install, &transaction.backup_install)
        .map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(&transaction.ready_install, &transaction.current_install) {
        let _ = std::fs::rename(&transaction.backup_install, &transaction.current_install);
        return Err(error.to_string());
    }
    Ok(())
}

fn rollback_install(transaction: &UpdateTransaction) -> Result<(), String> {
    if transaction.current_install.exists() {
        if transaction.ready_install.exists() {
            std::fs::remove_dir_all(&transaction.ready_install)
                .map_err(|error| error.to_string())?;
        }
        std::fs::rename(&transaction.current_install, &transaction.ready_install)
            .map_err(|error| error.to_string())?;
    }
    std::fs::rename(&transaction.backup_install, &transaction.current_install)
        .map_err(|error| error.to_string())
}

fn read_transaction(path: &Path) -> Result<UpdateTransaction, String> {
    if std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len()
        > 64 * 1024
    {
        return Err("更新事务文件超过安全上限".to_owned());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn validate_transaction_scope(path: &Path, transaction: &UpdateTransaction) -> Result<(), String> {
    let canonical_path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let expected_root =
        std::fs::canonicalize(transaction.workspace.join(".reactor/updates/transactions"))
            .map_err(|error| error.to_string())?;
    if !canonical_path.starts_with(&expected_root)
        || canonical_path.file_name() != Some(std::ffi::OsStr::new("transaction.json"))
    {
        return Err("更新事务路径越过 Reactor 受管目录".to_owned());
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    let launched_install = executable
        .ancestors()
        .find(|candidate| candidate.extension().and_then(std::ffi::OsStr::to_str) == Some("app"))
        .ok_or_else(|| "更新 Helper 不是从 Reactor.app 启动".to_owned())?;
    #[cfg(not(target_os = "macos"))]
    let launched_install = executable
        .parent()
        .ok_or_else(|| "更新 Helper 无法定位当前安装".to_owned())?;
    if std::fs::canonicalize(launched_install).map_err(|error| error.to_string())?
        != std::fs::canonicalize(&transaction.current_install).map_err(|error| error.to_string())?
    {
        return Err("更新事务的当前安装路径与运行中的 Reactor 不一致".to_owned());
    }
    Ok(())
}

fn wait_for_parent_exit(parent_pid: u32) -> Result<(), String> {
    for _ in 0..300 {
        #[cfg(unix)]
        let running = Command::new("kill")
            .args(["-0", &parent_pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        #[cfg(windows)]
        let running = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {parent_pid}"), "/NH"])
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout).contains(&parent_pid.to_string())
            });
        if !running {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("等待 Reactor 主进程退出超时，更新未执行".to_owned())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};

    #[test]
    fn version_comparison_is_numeric() {
        assert_eq!(
            compare_versions("1.10.0", "1.9.9").unwrap(),
            Ordering::Greater
        );
        assert_eq!(compare_versions("v1.2", "1.2.0").unwrap(), Ordering::Equal);
        assert!(compare_versions("latest", "1.0.0").is_err());
    }

    #[test]
    fn activation_and_rollback_restore_application_and_database() {
        let root = std::env::temp_dir().join(format!("reactor-updater-{}", Uuid::new_v4()));
        let current = root.join("Reactor.app");
        let ready = root.join(".Reactor-update.app");
        let backup = root.join(".Reactor-backup.app");
        let executable = candidate_executable(&current);
        let ready_executable = candidate_executable(&ready);
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(ready_executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, b"old").unwrap();
        std::fs::write(&ready_executable, b"new").unwrap();
        let database = root.join("runtime/reactor.sqlite3");
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        std::fs::write(&database, b"old-db").unwrap();
        let database_backup = root.join("database-backup");
        snapshot_database(&database, &database_backup).unwrap();
        let transaction = UpdateTransaction {
            transaction_id: "test".to_owned(),
            version: "1.0.1".to_owned(),
            phase: UpdatePhase::Staged,
            workspace: root.clone(),
            current_install: current.clone(),
            ready_install: ready.clone(),
            backup_install: backup,
            database_path: database.clone(),
            database_backup: database_backup.clone(),
            created_at: Utc::now(),
            error: None,
        };
        activate_install(&transaction).unwrap();
        assert_eq!(
            std::fs::read(candidate_executable(&current)).unwrap(),
            b"new"
        );
        std::fs::write(&database, b"migrated-db").unwrap();
        rollback_install(&transaction).unwrap();
        restore_database(&database, &database_backup).unwrap();
        assert_eq!(
            std::fs::read(candidate_executable(&current)).unwrap(),
            b"old"
        );
        assert_eq!(std::fs::read(database).unwrap(), b"old-db");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_rejects_parent_traversal() {
        let root = std::env::temp_dir().join(format!("reactor-archive-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(ensure_relative(Path::new("../escape")).is_err());
        assert!(ensure_relative(Path::new("Reactor.app/Contents")).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_hash_and_size_are_both_required() {
        let bytes = b"signed artifact";
        let path = std::env::temp_dir().join(format!("reactor-artifact-{}", Uuid::new_v4()));
        std::fs::write(&path, bytes).unwrap();
        let mut artifact = UpdateArtifact {
            platform: host_platform().to_owned(),
            arch: host_arch().to_owned(),
            url: "https://example.test/Reactor.app.tar.gz".to_owned(),
            sha256: hex::encode(Sha256::digest(bytes)),
            size: bytes.len() as u64,
        };
        verify_archive(&path, &artifact).unwrap();
        artifact.size += 1;
        assert!(verify_archive(&path, &artifact).is_err());
        artifact.size -= 1;
        artifact.sha256 = "0".repeat(64);
        assert!(verify_archive(&path, &artifact).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn host_artifact_must_match_platform_and_architecture() {
        let manifest = UpdateManifestV1 {
            schema_version: 1,
            channel: "stable".to_owned(),
            version: "1.0.1".to_owned(),
            published_at: "2026-08-19T00:00:00Z".to_owned(),
            compatibility: super::super::UpdateCompatibility {
                minimum_app_version: "0.1.0".to_owned(),
                database_schema: 2,
                flow_schemas: vec![1],
                result_schemas: vec![1],
            },
            artifacts: vec![UpdateArtifact {
                platform: "different-os".to_owned(),
                arch: host_arch().to_owned(),
                url: "https://example.test/Reactor.app.tar.gz".to_owned(),
                sha256: "a".repeat(64),
                size: 1,
            }],
            signature: super::super::UpdateSignature {
                algorithm: "Ed25519".to_owned(),
                key_id: "release-test".to_owned(),
                value: String::new(),
            },
        };
        assert!(select_host_artifact(&manifest).is_err());
    }

    #[test]
    fn verified_archive_is_staged_as_an_installable_transaction() {
        let root = std::env::temp_dir().join(format!("reactor-stage-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        let current = root.join("Applications/Reactor.app");
        let current_executable = candidate_executable(&current);
        std::fs::create_dir_all(current_executable.parent().unwrap()).unwrap();
        std::fs::write(&current_executable, b"old-version").unwrap();
        let candidate_bytes = b"new-version";
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o755);
        header.set_size(candidate_bytes.len() as u64);
        header.set_cksum();
        let relative = Path::new("Reactor.app").join(
            candidate_executable(Path::new(""))
                .strip_prefix("")
                .unwrap(),
        );
        archive
            .append_data(&mut header, relative, &candidate_bytes[..])
            .unwrap();
        let encoder = archive.into_inner().unwrap();
        let bytes = encoder.finish().unwrap();
        let archive_path = root.join("candidate.tar.gz");
        std::fs::write(&archive_path, &bytes).unwrap();
        let artifact = UpdateArtifact {
            platform: host_platform().to_owned(),
            arch: host_arch().to_owned(),
            url: "https://example.test/Reactor.app.tar.gz".to_owned(),
            sha256: hex::encode(Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        };
        let manifest = UpdateManifestV1 {
            schema_version: 1,
            channel: "stable".to_owned(),
            version: "1.0.1".to_owned(),
            published_at: "2026-08-19T00:00:00Z".to_owned(),
            compatibility: super::super::UpdateCompatibility {
                minimum_app_version: "0.1.0".to_owned(),
                database_schema: 2,
                flow_schemas: vec![1],
                result_schemas: vec![1],
            },
            artifacts: vec![artifact.clone()],
            signature: super::super::UpdateSignature {
                algorithm: "Ed25519".to_owned(),
                key_id: "release-test".to_owned(),
                value: String::new(),
            },
        };
        verify_archive(&archive_path, &artifact).unwrap();
        let staged =
            prepare_transaction(&workspace, &current, &manifest, &artifact, &archive_path).unwrap();
        let transaction = read_transaction(Path::new(&staged.transaction_path)).unwrap();
        assert_eq!(transaction.phase, UpdatePhase::Staged);
        assert_eq!(
            std::fs::read(candidate_executable(&transaction.ready_install)).unwrap(),
            candidate_bytes
        );
        assert_eq!(
            latest_transaction(&workspace).unwrap().phase,
            UpdatePhase::Staged
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

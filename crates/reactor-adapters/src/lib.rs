//! Adapter contracts. Built-ins use these traits; external plugins will use versioned JSON-RPC.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use reactor_protocol::{FlowLock, Platform};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub os_version: Option<String>,
    pub physical: bool,
}

#[derive(Debug, Clone)]
pub struct AutomationRequest<'a> {
    pub flow: &'a FlowLock,
    pub device: &'a Device,
    pub artifact_dir: &'a Path,
}

#[derive(Debug, Clone)]
pub struct CollectionRequest<'a> {
    pub app_id: &'a str,
    pub device: &'a Device,
    pub artifact_dir: &'a Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterArtifact {
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("adapter is unavailable: {0}")]
    Unavailable(String),
    #[error("adapter execution failed: {0}")]
    Execution(String),
    #[error("adapter produced invalid output: {0}")]
    InvalidOutput(String),
}

#[async_trait]
pub trait DeviceAdapter: Send + Sync {
    fn id(&self) -> &str;
    async fn discover(&self) -> Result<Vec<Device>, AdapterError>;
}

#[async_trait]
pub trait AutomationAdapter: Send + Sync {
    fn id(&self) -> &str;
    fn supports(&self, platform: Platform) -> bool;
    async fn dry_run(
        &self,
        request: AutomationRequest<'_>,
    ) -> Result<Vec<AdapterArtifact>, AdapterError>;
    async fn execute(
        &self,
        request: AutomationRequest<'_>,
    ) -> Result<Vec<AdapterArtifact>, AdapterError>;
}

#[async_trait]
pub trait CollectorAdapter: Send + Sync {
    fn id(&self) -> &str;
    fn supports(&self, platform: Platform) -> bool;
    async fn start(&self, request: CollectionRequest<'_>) -> Result<(), AdapterError>;
    async fn stop(
        &self,
        request: CollectionRequest<'_>,
    ) -> Result<Vec<AdapterArtifact>, AdapterError>;
}

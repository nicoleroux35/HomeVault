use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RescueError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Tool not available: {0}")]
    ToolNotAvailable(String),
    #[error("Job not found: {0}")]
    JobNotFound(String),
    #[error("Security error: {0}")]
    Security(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStatus {
    pub name: String,
    pub available: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockDevice {
    pub device: String,
    pub name: String,
    pub size: u64,
    pub model: String,
    pub transport: String,
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DdrescueStatus {
    pub job_id: String,
    pub progress: f64,
    pub message: String,
    pub bytes_copied: Option<u64>,
    pub bytes_total: Option<u64>,
    pub speed: Option<String>,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDiskResult {
    pub device: String,
    pub partitions: Vec<PartitionEntry>,
    pub log: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionEntry {
    pub partition: String,
    pub size: String,
    pub filesystem: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotoRecResult {
    pub device: String,
    pub output_dir: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdadmDiagnostic {
    pub array: String,
    pub level: String,
    pub state: String,
    pub devices: Vec<String>,
    pub size: String,
    pub health: String,
    pub rebuild_progress: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DdrescueOptions {
    pub logfile: Option<String>,
    pub block_size: Option<u32>,
    pub retry_passes: Option<u32>,
    pub direct_io: Option<bool>,
    pub reverse: Option<bool>,
    pub verbose: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestDiskOptions {
    pub deep_search: Option<bool>,
    pub log_path: Option<String>,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PhotoRecOptions {
    pub file_types: Option<Vec<String>>,
    pub keep_broken: Option<bool>,
    pub max_size: Option<u64>,
    pub partition_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MdadmOptions {
    pub action: Option<String>,
    pub force: Option<bool>,
    pub verbose: Option<bool>,
}

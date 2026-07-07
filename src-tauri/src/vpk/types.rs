use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct VpkEntry {
    pub crc: u32,
    pub preload_bytes: u16,
    pub archive_index: u16,
    pub entry_offset: u32,
    pub entry_length: u32,
    pub preload_data: Vec<u8>,
    pub header_size: u32,
    pub tree_size: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct AddonMetadata {
    #[serde(rename = "addonInfo")]
    pub addon_info: serde_json::Value,
    #[serde(rename = "hasImage")]
    pub has_image: bool,
    #[serde(rename = "imagePath")]
    pub image_path: Option<String>,
    #[serde(rename = "filesCount")]
    pub files_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub hash: String,
}

#[derive(Debug, Clone)]
pub struct VpkFileToWrite {
    pub ext: String,
    pub path: String,
    pub filename: String,
    pub content: Vec<u8>,
}

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use thiserror::Error;

const MAX_VPK_TREE_SIZE: u64 = 64 * 1024 * 1024;
const MAX_VPK_ENTRY_SIZE: u64 = 512 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum VpkReadError {
    #[error("无法打开 VPK 文件: {0}")]
    Io(#[from] std::io::Error),
    #[error("非合法 VPK 文件签名")]
    InvalidSignature,
    #[error("不支持的 VPK 版本: {0}")]
    UnsupportedVersion(u32),
    #[error("VPK 目录树损坏或超出边界")]
    CorruptedTree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub struct VpkArchive {
    pub version: u32,
    pub entries: BTreeMap<String, VpkEntry>,
    pub file: File,
}

pub(crate) fn read_c_string(buf: &[u8], offset: &mut usize) -> String {
    if *offset >= buf.len() {
        return String::new();
    }
    let start = *offset;
    while *offset < buf.len() && buf[*offset] != 0 {
        *offset += 1;
    }
    let s = String::from_utf8_lossy(&buf[start..*offset]).into_owned();
    if *offset < buf.len() {
        *offset += 1;
    }
    s
}

pub fn parse_vpk<P: AsRef<Path>>(file_path: P) -> Result<VpkArchive, VpkReadError> {
    let mut file = File::open(file_path)?;
    let mut prefix = [0u8; 12];
    file.read_exact(&mut prefix)?;

    let signature = u32::from_le_bytes(prefix[0..4].try_into().unwrap());
    if signature != 0x55aa1234 {
        return Err(VpkReadError::InvalidSignature);
    }

    let version = u32::from_le_bytes(prefix[4..8].try_into().unwrap());
    let tree_size = u32::from_le_bytes(prefix[8..12].try_into().unwrap());
    let header_size = match version {
        1 => 12,
        2 => {
            let mut v2_tail = [0u8; 16];
            file.read_exact(&mut v2_tail)?;
            28
        }
        other => return Err(VpkReadError::UnsupportedVersion(other)),
    };

    let file_len = file.metadata()?.len();
    if u64::from(tree_size) > MAX_VPK_TREE_SIZE
        || u64::from(header_size)
            .checked_add(u64::from(tree_size))
            .is_none_or(|end| end > file_len)
    {
        return Err(VpkReadError::CorruptedTree);
    }

    file.seek(SeekFrom::Start(header_size as u64))?;
    let mut tree_buf = vec![0u8; tree_size as usize];
    file.read_exact(&mut tree_buf)?;

    let mut offset = 0usize;
    let mut entries = BTreeMap::new();

    while offset < tree_buf.len() {
        let ext = read_c_string(&tree_buf, &mut offset);
        if ext.is_empty() {
            break;
        }

        while offset < tree_buf.len() {
            let path_str = read_c_string(&tree_buf, &mut offset);
            if path_str.is_empty() {
                break;
            }

            while offset < tree_buf.len() {
                let filename = read_c_string(&tree_buf, &mut offset);
                if filename.is_empty() {
                    break;
                }
                if offset + 18 > tree_buf.len() {
                    return Err(VpkReadError::CorruptedTree);
                }

                let slice = &tree_buf[offset..offset + 18];
                offset += 18;

                let crc = u32::from_le_bytes(slice[0..4].try_into().unwrap());
                let preload_bytes = u16::from_le_bytes(slice[4..6].try_into().unwrap());
                let archive_index = u16::from_le_bytes(slice[6..8].try_into().unwrap());
                let entry_offset = u32::from_le_bytes(slice[8..12].try_into().unwrap());
                let entry_length = u32::from_le_bytes(slice[12..16].try_into().unwrap());

                let preload_data = if preload_bytes > 0 {
                    let end = offset + preload_bytes as usize;
                    if end > tree_buf.len() {
                        return Err(VpkReadError::CorruptedTree);
                    }
                    let data = tree_buf[offset..end].to_vec();
                    offset = end;
                    data
                } else {
                    Vec::new()
                };

                let norm_path = path_str.trim();
                let ext_trimmed = ext.trim();
                let full_path = match (norm_path.is_empty(), ext_trimmed.is_empty()) {
                    (true, true) => filename,
                    (true, false) => format!("{filename}.{ext_trimmed}"),
                    (false, true) => format!("{norm_path}/{filename}"),
                    (false, false) => format!("{norm_path}/{filename}.{ext_trimmed}"),
                };

                entries.insert(
                    full_path,
                    VpkEntry {
                        crc,
                        preload_bytes,
                        archive_index,
                        entry_offset,
                        entry_length,
                        preload_data,
                        header_size,
                        tree_size,
                    },
                );
            }
        }
    }

    Ok(VpkArchive {
        version,
        entries,
        file,
    })
}

pub fn read_vpk_entry(file: &mut File, entry: &VpkEntry) -> std::io::Result<Vec<u8>> {
    let mut data = entry.preload_data.clone();
    if entry.entry_length > 0 && entry.archive_index == 0x7fff {
        let offset = entry.header_size as u64 + entry.tree_size as u64 + entry.entry_offset as u64;
        let file_len = file.metadata()?.len();
        if u64::from(entry.entry_length) > MAX_VPK_ENTRY_SIZE
            || offset
                .checked_add(u64::from(entry.entry_length))
                .is_none_or(|end| end > file_len)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "VPK 条目偏移或长度超出归档文件边界",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; entry.entry_length as usize];
        file.read_exact(&mut buf)?;
        data.extend_from_slice(&buf);
    }
    Ok(data)
}

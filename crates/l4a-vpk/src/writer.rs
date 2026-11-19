use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpkFileToWrite {
    pub ext: String,
    pub path: String,
    pub filename: String,
    pub content: Vec<u8>,
}

pub fn write_vpk<P: AsRef<Path>>(file_path: P, files: &[VpkFileToWrite]) -> Result<(), String> {
    if let Some(parent) = file_path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    let mut ext_map: BTreeMap<String, BTreeMap<String, Vec<&VpkFileToWrite>>> = BTreeMap::new();
    for f in files {
        ext_map
            .entry(f.ext.clone())
            .or_default()
            .entry(f.path.clone())
            .or_default()
            .push(f);
    }

    for paths in ext_map.values_mut() {
        for list in paths.values_mut() {
            list.sort_by(|a, b| a.filename.cmp(&b.filename));
        }
    }

    let mut tree_buf = Vec::new();
    let mut data_offset = 0u32;
    let mut data_blocks: Vec<&[u8]> = Vec::new();

    for (ext, paths) in &ext_map {
        let ext_str = if ext.is_empty() { " " } else { ext.as_str() };
        tree_buf.extend_from_slice(ext_str.as_bytes());
        tree_buf.push(0);

        for (path, file_list) in paths {
            let path_str = if path.is_empty() { " " } else { path.as_str() };
            tree_buf.extend_from_slice(path_str.as_bytes());
            tree_buf.push(0);

            for f in file_list {
                let filename_str = if f.filename.is_empty() {
                    " "
                } else {
                    f.filename.as_str()
                };
                tree_buf.extend_from_slice(filename_str.as_bytes());
                tree_buf.push(0);

                let crc_val = crc32fast::hash(&f.content);
                let entry_length = u32::try_from(f.content.len())
                    .map_err(|_| format!("文件超出 VPK 单文件 4GiB 限制: {}", f.filename))?;

                tree_buf.extend_from_slice(&crc_val.to_le_bytes());
                tree_buf.extend_from_slice(&0u16.to_le_bytes());
                tree_buf.extend_from_slice(&0x7fffu16.to_le_bytes());
                tree_buf.extend_from_slice(&data_offset.to_le_bytes());
                tree_buf.extend_from_slice(&entry_length.to_le_bytes());
                tree_buf.extend_from_slice(&0xffffu16.to_le_bytes());

                data_offset = data_offset
                    .checked_add(entry_length)
                    .ok_or_else(|| "VPK 数据体总大小超出 u32 范围".to_string())?;
                data_blocks.push(&f.content);
            }
            tree_buf.push(0);
        }
        tree_buf.push(0);
    }
    tree_buf.push(0);

    let tree_size = u32::try_from(tree_buf.len()).map_err(|_| "VPK 目录树过大".to_string())?;
    let mut file = File::create(file_path).map_err(|e| e.to_string())?;
    file.write_all(&0x55aa1234u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&1u32.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&tree_size.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(&tree_buf).map_err(|e| e.to_string())?;

    for block in data_blocks {
        file.write_all(block).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;

    Ok(())
}

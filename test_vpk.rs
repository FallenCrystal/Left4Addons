fn read_string(buf: &[u8], offset: &mut usize) -> String {
    let start = *offset;
    while *offset < buf.len() && buf[*offset] != 0 {
        *offset += 1;
    }
    let s = String::from_utf8_lossy(&buf[start..*offset]).into_owned();
    *offset += 1; // skip null terminator
    s
}

fn dump_vpk(file_path: &str) {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(file_path).unwrap();
    let mut header_buf = vec![0; 28];
    file.read_exact(&mut header_buf).unwrap();

    let signature = u32::from_le_bytes(header_buf[0..4].try_into().unwrap());
    if signature != 0x55aa1234 {
        println!("Not a VPK");
        return;
    }

    let version = u32::from_le_bytes(header_buf[4..8].try_into().unwrap());
    let tree_size: u32;
    let header_size: u32;

    if version == 1 {
        tree_size = u32::from_le_bytes(header_buf[8..12].try_into().unwrap());
        header_size = 12;
    } else if version == 2 {
        tree_size = u32::from_le_bytes(header_buf[8..12].try_into().unwrap());
        header_size = 28;
    } else {
        return;
    }

    file.seek(SeekFrom::Start(header_size as u64)).unwrap();
    let mut tree_buf = vec![0; tree_size as usize];
    file.read_exact(&mut tree_buf).unwrap();

    let mut offset = 0;
    while offset < tree_buf.len() {
        let ext = read_string(&tree_buf, &mut offset);
        if ext.is_empty() { break; }

        while offset < tree_buf.len() {
            let path_str = read_string(&tree_buf, &mut offset);
            if path_str.is_empty() { break; }

            while offset < tree_buf.len() {
                let filename = read_string(&tree_buf, &mut offset);
                if filename.is_empty() { break; }

                if offset + 18 > tree_buf.len() { break; }
                offset += 18;
                let preload_bytes = u16::from_le_bytes(tree_buf[offset-14..offset-12].try_into().unwrap());
                if preload_bytes > 0 {
                    offset += preload_bytes as usize;
                }

                let norm_path = path_str.trim();
                let full_path = if norm_path.is_empty() {
                    format!("{}.{}", filename, ext)
                } else {
                    format!("{}/{}.{}", norm_path, filename, ext)
                };

                if full_path.to_lowercase().contains("addonimage") || full_path.to_lowercase().contains("addoninfo") {
                    println!("{}: {:?}", file_path, full_path);
                }
            }
        }
    }
}

fn main() {
    let dir = std::fs::read_dir("example-addons").unwrap();
    for entry in dir {
        let entry = entry.unwrap();
        dump_vpk(entry.path().to_str().unwrap());
    }
}

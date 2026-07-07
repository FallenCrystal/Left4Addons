mod core;
#[cfg(test)]
mod tests;
mod types;

pub use core::{
    crc32, extract_addon_metadata, generate_dummy_vpk, get_file_content, parse_key_values,
    parse_vpk, write_vpk,
};
pub use types::{AddonMetadata, VpkEntry, VpkFileToWrite};

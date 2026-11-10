//! Windows file-attribute and reparse-tag values, and how they map to our
//! node flags. Pure numbers, no FFI, so every scanner can share them.

use crate::flags;

pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
pub const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x0000_0200;
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
pub const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0000_0800;
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// Symlinks and junctions have this bit set ("name surrogate").
pub const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;
/// IO_REPARSE_TAG_CLOUD and CLOUD_1..CLOUD_F (bits 12-15 vary).
pub const REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
pub const REPARSE_TAG_CLOUD_MASK: u32 = 0xFFFF_0FFF;

/// Maps Windows file attributes plus reparse tag (0 if none) to node flags.
pub fn node_flags(attrs: u32, reparse_tag: u32) -> u8 {
    let mut f = 0;
    if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        f |= flags::DIR;
    }
    // Only symlinks/junctions are "links". Other reparse points, such as
    // OneDrive folders, are real folders and must be scanned.
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse_tag & REPARSE_TAG_NAME_SURROGATE != 0 {
        f |= flags::LINK;
    }
    let cloud_attrs = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;
    if attrs & cloud_attrs != 0 || reparse_tag & REPARSE_TAG_CLOUD_MASK == REPARSE_TAG_CLOUD {
        f |= flags::CLOUD;
    }
    if attrs & FILE_ATTRIBUTE_COMPRESSED != 0 {
        f |= flags::COMPRESSED;
    }
    if attrs & FILE_ATTRIBUTE_SPARSE_FILE != 0 {
        f |= flags::SPARSE;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_reparse_points() {
        const JUNCTION: u32 = 0xA000_0003;
        const SYMLINK: u32 = 0xA000_000C;
        const ONEDRIVE: u32 = 0x9000_601A; // IO_REPARSE_TAG_CLOUD_6
        const DIR_REPARSE: u32 = FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT;

        assert_eq!(node_flags(DIR_REPARSE, JUNCTION), flags::DIR | flags::LINK);
        assert_eq!(node_flags(FILE_ATTRIBUTE_REPARSE_POINT, SYMLINK), flags::LINK);
        // OneDrive folder: cloud, but NOT a link, so it gets scanned.
        assert_eq!(node_flags(DIR_REPARSE, ONEDRIVE), flags::DIR | flags::CLOUD);
        assert_eq!(node_flags(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, 0), flags::CLOUD);
        assert_eq!(node_flags(0, 0), 0);
    }
}

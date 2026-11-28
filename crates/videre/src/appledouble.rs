//! Recognize AppleDouble metadata by its contents, independently of its name.

pub const HEADER_LEN: usize = 26;

/// Identify the container from its magic and supported version, checking that
/// the fixed header and declared entry table fit in the file. Resource contents
/// are not validated. Uncertain headers and AppleSingle remain indexable.
pub fn is_header(head: &[u8], file_len: u64) -> bool {
    if head.len() < HEADER_LEN || !head.starts_with(b"\x00\x05\x16\x07") {
        return false;
    }
    if !matches!(&head[4..8], b"\x00\x01\x00\x00" | b"\x00\x02\x00\x00") {
        return false;
    }
    let entries = u16::from_be_bytes([head[24], head[25]]);
    HEADER_LEN as u64 + u64::from(entries) * 12 <= file_len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: u32, entries: u16) -> [u8; HEADER_LEN] {
        let mut head = [0; HEADER_LEN];
        head[..4].copy_from_slice(&0x0005_1607u32.to_be_bytes());
        head[4..8].copy_from_slice(&version.to_be_bytes());
        head[24..].copy_from_slice(&entries.to_be_bytes());
        head
    }

    #[test]
    fn appledouble_header_detection_preserves_other_formats_and_uncertain_data() {
        let v1 = header(0x0001_0000, 1);
        let v2 = header(0x0002_0000, 1);
        let empty = header(0x0002_0000, 0);
        let unknown = header(0x0003_0000, 0);
        let mut single = v2;
        single[..4].copy_from_slice(&0x0005_1600u32.to_be_bytes());
        let mut jpeg = v2;
        jpeg[..3].copy_from_slice(b"\xff\xd8\xff");

        let cases: [(&str, &[u8], u64, bool); 8] = [
            ("version 1", &v1, 70, true),
            ("version 2", &v2, 70, true),
            ("no entries", &empty, 26, true),
            ("short header", &v2[..25], 70, false),
            ("truncated entry table", &v2, 37, false),
            ("unknown version", &unknown, 70, false),
            ("AppleSingle", &single, 70, false),
            ("JPEG", &jpeg, 70, false),
        ];
        for (name, head, file_len, expected) in cases {
            assert_eq!(is_header(head, file_len), expected, "{name}");
        }
    }
}

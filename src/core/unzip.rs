//! Takes one file out of a release zip, in memory: the central directory is read from the end, the entry's CRC-32 and
//! sizes are checked, and only stored or deflated entries are accepted (no encryption, no ZIP64, no folders in the
//! name). Nothing is written to disk here, so a bad archive can never leave half a file behind.

#[derive(Debug, PartialEq, Eq)]
pub enum UnzipProblem {
    NotAZip,
    Missing(String),
    Unsupported(&'static str),
    TooLarge,
    Damaged(&'static str),
}

impl std::fmt::Display for UnzipProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnzipProblem::NotAZip => write!(f, "the download is not a zip archive"),
            UnzipProblem::Missing(n) => write!(f, "{n} is not in the archive"),
            UnzipProblem::Unsupported(w) => write!(f, "the archive uses {w}"),
            UnzipProblem::TooLarge => write!(f, "the file in the archive is too large"),
            UnzipProblem::Damaged(w) => write!(f, "the archive is damaged ({w})"),
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> { b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])) }
fn u32_at(b: &[u8], at: usize) -> Option<u32> { b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])) }

pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 { c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 }; }
        *t = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data { crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8); }
    !crc
}

/// End of central directory: (entries, directory offset). Searched backwards past an archive comment.
fn end_record(zip: &[u8]) -> Option<(usize, usize)> {
    if zip.len() < 22 { return None; }
    let lowest = zip.len().saturating_sub(22 + 65_535);
    let mut at = zip.len() - 22;
    loop {
        if u32_at(zip, at) == Some(0x0605_4b50) {
            let entries = u16_at(zip, at + 10)? as usize;
            let offset = u32_at(zip, at + 16)? as usize;
            return Some((entries, offset));
        }
        if at == lowest { return None; }
        at -= 1;
    }
}

/// The bytes of the top-level file `name` (letter case ignored, as on Windows). At most `limit` bytes unpacked.
pub fn extract(zip: &[u8], name: &str, limit: usize) -> Result<Vec<u8>, UnzipProblem> {
    let (entries, mut at) = end_record(zip).ok_or(UnzipProblem::NotAZip)?;
    for _ in 0..entries {
        if u32_at(zip, at) != Some(0x0201_4b50) { return Err(UnzipProblem::Damaged("central directory")); }
        let flags = u16_at(zip, at + 8).ok_or(UnzipProblem::Damaged("central directory"))?;
        let method = u16_at(zip, at + 10).ok_or(UnzipProblem::Damaged("central directory"))?;
        let crc = u32_at(zip, at + 16).ok_or(UnzipProblem::Damaged("central directory"))?;
        let packed = u32_at(zip, at + 20).ok_or(UnzipProblem::Damaged("central directory"))?;
        let size = u32_at(zip, at + 24).ok_or(UnzipProblem::Damaged("central directory"))?;
        let n = u16_at(zip, at + 28).ok_or(UnzipProblem::Damaged("central directory"))? as usize;
        let e = u16_at(zip, at + 30).ok_or(UnzipProblem::Damaged("central directory"))? as usize;
        let c = u16_at(zip, at + 32).ok_or(UnzipProblem::Damaged("central directory"))? as usize;
        let local = u32_at(zip, at + 42).ok_or(UnzipProblem::Damaged("central directory"))? as usize;
        let entry_name = zip.get(at + 46..at + 46 + n).ok_or(UnzipProblem::Damaged("central directory"))?;
        at += 46 + n + e + c;
        // Only a file at the top of the archive: "x/NachoCombo.exe" or "..\NachoCombo.exe" is a different file.
        if !entry_name.eq_ignore_ascii_case(name.as_bytes()) { continue; }
        if flags & 1 != 0 { return Err(UnzipProblem::Unsupported("encryption")); }
        if packed == u32::MAX || size == u32::MAX { return Err(UnzipProblem::Unsupported("ZIP64")); }
        if size as usize > limit { return Err(UnzipProblem::TooLarge); }
        if u32_at(zip, local) != Some(0x0403_4b50) { return Err(UnzipProblem::Damaged("local header")); }
        let ln = u16_at(zip, local + 26).ok_or(UnzipProblem::Damaged("local header"))? as usize;
        let le = u16_at(zip, local + 28).ok_or(UnzipProblem::Damaged("local header"))? as usize;
        let start = local + 30 + ln + le;
        let data = zip.get(start..start + packed as usize).ok_or(UnzipProblem::Damaged("file data"))?;
        let out = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, limit).map_err(|e| {
                if matches!(e.status, miniz_oxide::inflate::TINFLStatus::HasMoreOutput) { UnzipProblem::TooLarge } else { UnzipProblem::Damaged("deflate stream") }
            })?,
            _ => return Err(UnzipProblem::Unsupported("a compression method other than deflate")),
        };
        if out.len() != size as usize { return Err(UnzipProblem::Damaged("size")); }
        if crc32(&out) != crc { return Err(UnzipProblem::Damaged("CRC-32")); }
        return Ok(out);
    }
    Err(UnzipProblem::Missing(name.to_string()))
}

/// Builds a zip in memory (stored or deflated entries). Tests and the update round trip use it to make fake releases.
#[allow(dead_code)]
pub fn build(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut out = vec![];
    let mut dir = vec![];
    for (name, data, deflate) in files {
        let packed = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
        let method: u16 = if *deflate { 8 } else { 0 };
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);
        dir.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        dir.extend_from_slice(&20u16.to_le_bytes());
        dir.extend_from_slice(&20u16.to_le_bytes());
        dir.extend_from_slice(&0u16.to_le_bytes());
        dir.extend_from_slice(&method.to_le_bytes());
        dir.extend_from_slice(&[0, 0, 0x21, 0]);
        dir.extend_from_slice(&crc.to_le_bytes());
        dir.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
        dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
        dir.extend_from_slice(&[0; 12]);
        dir.extend_from_slice(&offset.to_le_bytes());
        dir.extend_from_slice(name.as_bytes());
    }
    let dir_at = out.len() as u32;
    out.extend_from_slice(&dir);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const EXE: &[u8] = b"MZ pretend program, long enough to be worth deflating: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test] fn crc32_check_value() { assert_eq!(crc32(b"123456789"), 0xCBF4_3926); }

    #[test] fn takes_the_exe_out_of_a_deflated_release_zip() {
        let zip = build(&[("README.md", b"read me", true), ("NachoCombo.exe", EXE, true), ("LICENSE", b"MIT", false)]);
        assert_eq!(extract(&zip, "NachoCombo.exe", 1 << 20).unwrap(), EXE);
        assert_eq!(extract(&zip, "nachocombo.EXE", 1 << 20).unwrap(), EXE, "letter case does not matter on Windows");
        assert_eq!(extract(&zip, "LICENSE", 1 << 20).unwrap(), b"MIT");
    }

    #[test] fn a_file_in_a_folder_is_not_the_exe() {
        let zip = build(&[("x/NachoCombo.exe", EXE, true), ("../NachoCombo.exe", EXE, false)]);
        assert_eq!(extract(&zip, "NachoCombo.exe", 1 << 20), Err(UnzipProblem::Missing("NachoCombo.exe".into())));
    }

    #[test] fn refuses_damaged_or_odd_archives() {
        assert_eq!(extract(b"MZ not a zip at all, just bytes......", "NachoCombo.exe", 1 << 20), Err(UnzipProblem::NotAZip));
        // One flipped byte in the stored data: the CRC catches it.
        let mut zip = build(&[("NachoCombo.exe", EXE, false)]);
        zip[30 + "NachoCombo.exe".len() + 3] ^= 0xFF;
        assert_eq!(extract(&zip, "NachoCombo.exe", 1 << 20), Err(UnzipProblem::Damaged("CRC-32")));
        // Encrypted entry.
        let mut zip = build(&[("NachoCombo.exe", EXE, false)]);
        let cd = names_offset(&zip);
        zip[cd + 8] |= 1;
        assert_eq!(extract(&zip, "NachoCombo.exe", 1 << 20), Err(UnzipProblem::Unsupported("encryption")));
        // Cut short.
        let zip = build(&[("NachoCombo.exe", EXE, true)]);
        assert!(extract(&zip[..zip.len() - 30], "NachoCombo.exe", 1 << 20).is_err());
    }

    #[test] fn a_zip_bomb_stops_at_the_limit() {
        let big = vec![0u8; 4 << 20];
        let zip = build(&[("NachoCombo.exe", &big, true)]);
        assert!(zip.len() < 64 * 1024);
        assert_eq!(extract(&zip, "NachoCombo.exe", 1 << 20), Err(UnzipProblem::TooLarge));
        assert_eq!(extract(&zip, "NachoCombo.exe", 8 << 20).unwrap().len(), 4 << 20);
    }

    fn names_offset(zip: &[u8]) -> usize { end_record(zip).unwrap().1 }
}

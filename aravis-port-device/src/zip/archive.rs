use super::deflate::inflate;

#[derive(Debug, thiserror::Error)]
pub enum ZipError {
    #[error("not a zip local-file-header (missing PK\\x03\\x04 signature)")]
    NotAZip,
    #[error("truncated zip entry")]
    Truncated,
    #[error("zip entries using a trailing data descriptor (streaming mode) are not supported")]
    StreamingModeUnsupported,
    #[error("unsupported zip compression method {0} (only store=0 and deflate=8 are supported)")]
    UnsupportedMethod(u16),
    #[error("inflate error: {0}")]
    Inflate(#[from] super::deflate::InflateError),
}

const LOCAL_FILE_HEADER_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
const DATA_DESCRIPTOR_FLAG: u16 = 0x08;
const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// Extract the first file entry from a ZIP archive's bytes. GenICam XML archives (confirmed
/// against the live AT-Automation Technology C5-2040-GigE camera) contain exactly one file, so
/// this doesn't parse the central directory — it reads the first local file header directly,
/// which is always present at the start of a ZIP produced by standard tools.
pub fn extract_first_file(data: &[u8]) -> Result<Vec<u8>, ZipError> {
    if data.len() < 30 || data[0..4] != LOCAL_FILE_HEADER_SIGNATURE {
        return Err(ZipError::NotAZip);
    }
    let flags = u16::from_le_bytes([data[6], data[7]]);
    let method = u16::from_le_bytes([data[8], data[9]]);
    let compressed_size = u32::from_le_bytes(data[18..22].try_into().unwrap()) as usize;
    let uncompressed_size = u32::from_le_bytes(data[22..26].try_into().unwrap()) as usize;
    let name_len = u16::from_le_bytes([data[26], data[27]]) as usize;
    let extra_len = u16::from_le_bytes([data[28], data[29]]) as usize;

    if flags & DATA_DESCRIPTOR_FLAG != 0 {
        return Err(ZipError::StreamingModeUnsupported);
    }

    let data_start = 30usize
        .checked_add(name_len)
        .and_then(|v| v.checked_add(extra_len))
        .ok_or(ZipError::Truncated)?;
    let compressed = data
        .get(data_start..data_start + compressed_size)
        .ok_or(ZipError::Truncated)?;

    match method {
        METHOD_STORE => Ok(compressed.to_vec()),
        METHOD_DEFLATE => Ok(inflate(compressed, Some(uncompressed_size))?),
        other => Err(ZipError::UnsupportedMethod(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_zip_entry(name: &[u8], content: &[u8]) -> Vec<u8> {
        let mut zip = Vec::new();
        zip.extend_from_slice(&LOCAL_FILE_HEADER_SIGNATURE);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&METHOD_STORE.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&0u32.to_le_bytes()); // crc32 (unchecked by our reader)
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes()); // compressed size
        zip.extend_from_slice(&(content.len() as u32).to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra len
        zip.extend_from_slice(name);
        zip.extend_from_slice(content);
        zip
    }

    #[test]
    fn extracts_a_stored_entry() {
        let zip = stored_zip_entry(b"camera.xml", b"<RegisterDescription/>");
        assert_eq!(extract_first_file(&zip).unwrap(), b"<RegisterDescription/>");
    }

    #[test]
    fn rejects_non_zip_input() {
        assert!(matches!(
            extract_first_file(b"not a zip"),
            Err(ZipError::NotAZip)
        ));
    }

    #[test]
    fn rejects_truncated_entries_without_panicking() {
        let mut zip = stored_zip_entry(b"a.xml", b"0123456789");
        zip.truncate(zip.len() - 5); // chop off part of the content
        assert!(matches!(extract_first_file(&zip), Err(ZipError::Truncated)));
    }
}

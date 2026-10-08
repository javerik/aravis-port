use aravis_port_core::bootstrap::offset;
use aravis_port_core::{Error, Result};

use crate::net::GvcpTransaction;
use crate::zip;

/// Fetch the GenICam XML document, trying `XML_URL_0` then `XML_URL_1`, per the standard
/// `"local:path;address;size"` URL convention (case-insensitive scheme). A `.zip` path suffix is
/// unzipped via our own pure-Rust DEFLATE decoder (see [`crate::zip`]) — confirmed necessary
/// against the live C5-2040-GigE camera, which serves its XML this way. `file:` URLs are
/// recognized but rejected: they don't apply to network cameras.
pub(crate) fn fetch(txn: &mut GvcpTransaction) -> Result<Vec<u8>> {
    let url = match read_url(txn, offset::XML_URL_0) {
        Ok(url) => url,
        Err(_) => read_url(txn, offset::XML_URL_1)?,
    };
    fetch_from_url(txn, &url)
}

fn read_url(txn: &mut GvcpTransaction, address: u32) -> Result<String> {
    let bytes = txn.read_memory(address, offset::XML_URL_LEN)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    if s.is_empty() {
        Err(Error::GenIcam("empty XML URL register".to_string()))
    } else {
        Ok(s)
    }
}

fn fetch_from_url(txn: &mut GvcpTransaction, url: &str) -> Result<Vec<u8>> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("local:") {
        // `to_ascii_lowercase` never changes a string's byte length, so slicing the original
        // (case-preserved) `url` at the same byte offset as the lowercased prefix is safe.
        let original_rest = &url["local:".len()..];
        let parts: Vec<&str> = original_rest.splitn(3, ';').collect();
        let [path, address_hex, size_hex] = parts.as_slice() else {
            return Err(Error::GenIcam(format!("malformed 'local:' XML url: {url}")));
        };
        let address = parse_hex_u32(address_hex)?;
        let size = parse_hex_u32(size_hex)? as usize;
        let data = txn.read_memory(address, size)?;
        if path.to_ascii_lowercase().ends_with(".zip") {
            zip::extract_first_file(&data)
                .map_err(|e| Error::GenIcam(format!("failed to unzip GenICam XML: {e}")))
        } else {
            Ok(data)
        }
    } else if lower.starts_with("file:") {
        Err(Error::GenIcam(format!(
            "'file:' XML urls are not supported for network cameras: {url}"
        )))
    } else {
        Err(Error::GenIcam(format!("unsupported XML url scheme: {url}")))
    }
}

fn parse_hex_u32(text: &str) -> Result<u32> {
    let t = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u32::from_str_radix(t, 16)
        .map_err(|_| Error::GenIcam(format!("invalid hex value '{text}' in XML url")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_prefix() {
        assert_eq!(parse_hex_u32("0x1000").unwrap(), 0x1000);
        assert_eq!(parse_hex_u32("1000").unwrap(), 0x1000);
    }
}

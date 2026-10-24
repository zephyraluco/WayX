//! Bounded local Xauthority parsing; never logs credential data.
use anyhow::{Context, Result, anyhow, ensure};
use std::path::Path;
pub fn xauthority_cookie(path: &Path, display: u16) -> Result<[u8; 16]> {
    let bytes = std::fs::read(path).context("Read local Xauthority file")?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Xauthority file exceeds size limit"
    );
    let mut cursor = 0;
    let number = display.to_string();
    fn field<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8]> {
        let size = bytes
            .get(*cursor..*cursor + 2)
            .context("Truncated Xauthority length")?;
        let size = u16::from_be_bytes([size[0], size[1]]) as usize;
        *cursor += 2;
        let value = bytes
            .get(*cursor..*cursor + size)
            .context("Truncated Xauthority value")?;
        *cursor += size;
        Ok(value)
    }
    while cursor < bytes.len() {
        let family = bytes
            .get(cursor..cursor + 2)
            .context("Truncated Xauthority family")?;
        let family = u16::from_be_bytes([family[0], family[1]]);
        cursor += 2;
        let address = field(&bytes, &mut cursor)?;
        let n = field(&bytes, &mut cursor)?;
        let name = field(&bytes, &mut cursor)?;
        let cookie = field(&bytes, &mut cursor)?;
        let local = family == 256 || family == 65535 || (family == 0 && address == [127, 0, 0, 1]);
        if local && n == number.as_bytes() && name == b"MIT-MAGIC-COOKIE-1" && cookie.len() == 16 {
            return Ok(cookie.try_into().unwrap());
        }
    }
    Err(anyhow!(
        "No local MIT-MAGIC-COOKIE-1 entry for display :{display}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_and_oversized_xauthority() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), [1, 0, 255]).unwrap();
        assert!(xauthority_cookie(file.path(), 0).is_err());
        file.as_file().set_len(1024 * 1024 + 1).unwrap();
        assert!(xauthority_cookie(file.path(), 0).is_err());
    }
}

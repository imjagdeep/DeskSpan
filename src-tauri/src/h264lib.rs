//! Cisco's OpenH264 library, the H.264 encoder.
//!
//! Cisco pays the H.264 patent licence for its own prebuilt library, but only
//! when that exact binary is downloaded from Cisco, not bundled. So DeskSpan
//! fetches it from ciscobinary.openh264.org the first time it's needed
//! (about 1 MB), keeps it in the app's data folder, and loads it only if its
//! SHA-256 is one of the published releases (openh264 crate's own list).
//!
//! "OpenH264 Video Codec provided by Cisco Systems, Inc."

use std::io::Read;
use std::path::{Path, PathBuf};

const VERSION: &str = "2.6.0";

/// The library's file name for this computer, as Cisco publishes it.
pub fn file_name() -> Option<String> {
    let name = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "openh264-{v}-win64.dll",
        ("windows", "aarch64") => "openh264-{v}-win-arm64.dll",
        ("macos", "aarch64") => "libopenh264-{v}-mac-arm64.dylib",
        ("macos", "x86_64") => "libopenh264-{v}-mac-x64.dylib",
        ("linux", "x86_64") => "libopenh264-{v}-linux64.8.so",
        ("linux", "aarch64") => "libopenh264-{v}-linux-arm64.8.so",
        _ => return None,
    };
    Some(name.replace("{v}", VERSION))
}

/// The checked library's path, downloading it from Cisco first if needed.
/// (The encoder thread loads it from there with [`load`].)
pub async fn ensure(data_dir: &Path) -> Result<PathBuf, String> {
    let file = file_name().ok_or("no H.264 encoder from Cisco for this kind of computer")?;
    let dest = data_dir.join("openh264").join(&file);
    if dest.exists() {
        match load(&dest) {
            Ok(_) => return Ok(dest),
            // Damaged or replaced: fetch it again.
            Err(e) => tracing::warn!("{} not usable ({e}), downloading again", dest.display()),
        }
    }
    download(&file, &dest).await?;
    if let Err(e) = load(&dest) {
        let _ = std::fs::remove_file(&dest);
        return Err(format!("the encoder downloaded from Cisco didn't pass the check: {e}"));
    }
    Ok(dest)
}

/// Load the library; refused unless its SHA-256 is a published Cisco build.
pub fn load(path: &Path) -> Result<openh264::OpenH264API, String> {
    openh264::OpenH264API::from_blob_path(path).map_err(|e| e.to_string())
}

async fn download(file: &str, dest: &Path) -> Result<(), String> {
    // Cisco serves these over plain HTTP; the SHA-256 check after download
    // is what makes it safe.
    let url = format!("http://ciscobinary.openh264.org/{file}.bz2");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("cannot download the encoder: {e}"))?;
    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("cannot download the encoder from Cisco ({e}); check the internet connection"))?;
    if !res.status().is_success() {
        return Err(format!(
            "cannot download the encoder from Cisco (HTTP {})",
            res.status()
        ));
    }
    let packed = res
        .bytes()
        .await
        .map_err(|e| format!("the encoder download broke off: {e}"))?;
    let lib = unpack(&packed)?;
    let dir = dest.parent().ok_or("bad encoder folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &lib).map_err(|e| format!("cannot save the encoder: {e}"))?;
    std::fs::rename(&tmp, dest).map_err(|e| format!("cannot save the encoder: {e}"))
}

/// bzip2 → the library, refusing anything implausibly large.
fn unpack(packed: &[u8]) -> Result<Vec<u8>, String> {
    const MAX: u64 = 20 * 1024 * 1024;
    let mut out = Vec::new();
    bzip2::read::BzDecoder::new(packed)
        .take(MAX + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("the encoder download is damaged: {e}"))?;
    if out.len() as u64 > MAX {
        return Err("the encoder download is too large".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computer_has_a_cisco_build() {
        // CI runs on Windows x64, macOS arm64 and Linux x64.
        let name = file_name().expect("supported platform");
        assert!(name.contains(VERSION));
    }

    #[test]
    fn unpack_round_trip_and_garbage() {
        use std::io::Write;
        let mut enc = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        enc.write_all(b"library bytes").unwrap();
        let packed = enc.finish().unwrap();
        assert_eq!(unpack(&packed).unwrap(), b"library bytes");
        assert!(unpack(b"not bzip2").is_err());
    }
}

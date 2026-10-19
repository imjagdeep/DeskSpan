//! Android phones by USB cable, through Google's adb ("USB debugging").
//!
//! macOS can't use a Samsung's USB tethering (it speaks RNDIS), so the
//! cable link goes through adb instead: `adb reverse` makes the phone's
//! `localhost:<port>` reach this computer's, for the page and signalling
//! and for each picture connection's TCP port. Browsers also treat
//! `localhost` as a secure page.
//!
//! An adb already on the computer (Android Studio, Homebrew, PATH) is used;
//! otherwise Google's platform-tools are downloaded once from
//! dl.google.com (over HTTPS) into DeskSpan's data folder, after the user
//! clicks "Set up".

use serde::Serialize;
use std::collections::{BTreeSet, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// One phone adb can see.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsbPhone {
    pub serial: String,
    /// "Galaxy S24" style name when the phone tells it.
    pub model: String,
    /// "ready", "allow" (tap Allow on the phone), "offline".
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsbView {
    /// "off", "preparing", "ready", "error"
    pub setup: &'static str,
    pub error: Option<String>,
    pub phones: Vec<UsbPhone>,
}

#[derive(Default)]
struct Inner {
    adb: Option<PathBuf>,
    preparing: bool,
    error: Option<String>,
    phones: Vec<UsbPhone>,
    /// Picture connections' TCP ports to forward too.
    ports: BTreeSet<u16>,
    /// Phones whose browser was opened at DeskSpan this sharing session.
    opened: HashSet<String>,
}

#[derive(Default)]
pub struct Usb {
    inner: Mutex<Inner>,
}

impl Usb {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn view(&self) -> UsbView {
        let g = self.lock();
        UsbView {
            setup: if g.preparing {
                "preparing"
            } else if g.adb.is_some() {
                "ready"
            } else if g.error.is_some() {
                "error"
            } else {
                "off"
            },
            error: g.error.clone(),
            phones: g.phones.clone(),
        }
    }

    pub fn ready(&self) -> bool {
        self.lock().adb.is_some()
    }

    /// Find adb, or download Google's platform-tools. Blocking.
    pub fn setup(&self, data_dir: &Path) -> Result<(), String> {
        {
            let mut g = self.lock();
            if g.adb.is_some() {
                return Ok(());
            }
            g.preparing = true;
            g.error = None;
        }
        let found = find_adb(data_dir)
            .map(Ok)
            .unwrap_or_else(|| download(data_dir));
        let mut g = self.lock();
        g.preparing = false;
        match found {
            Ok(path) => {
                tracing::info!("adb: {}", path.display());
                g.adb = Some(path);
                Ok(())
            }
            Err(e) => {
                g.error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Forget adb (turned off in the window); phones are released.
    pub fn turn_off(&self) {
        let adb = {
            let mut g = self.lock();
            g.phones.clear();
            g.opened.clear();
            g.adb.take()
        };
        if let Some(adb) = adb {
            let _ = run(&adb, &["reverse", "--remove-all"]);
        }
    }

    /// One look at the cable (every few seconds while sharing; blocking):
    /// list phones, forward ports to ready ones, open DeskSpan on a phone
    /// the first time it's seen. `port` None = sharing stopped.
    pub fn tick(&self, port: Option<u16>, pair_url_path: Option<String>) {
        let Some(adb) = self.lock().adb.clone() else {
            return;
        };
        let Some(port) = port else {
            let mut g = self.lock();
            g.opened.clear();
            g.phones.clear();
            return;
        };
        let phones = match run(&adb, &["devices", "-l"]) {
            Ok(out) => parse_devices(&out),
            Err(e) => {
                tracing::debug!("adb devices: {e}");
                Vec::new()
            }
        };
        let ports: Vec<u16> = self.lock().ports.iter().copied().collect();
        for phone in phones.iter().filter(|p| p.state == "ready") {
            for p in std::iter::once(port).chain(ports.iter().copied()) {
                let spec = format!("tcp:{p}");
                if let Err(e) = run(&adb, &["-s", &phone.serial, "reverse", &spec, &spec]) {
                    tracing::warn!("adb reverse {p} on {}: {e}", phone.serial);
                }
            }
            let first = self.lock().opened.insert(phone.serial.clone());
            if first {
                // Open the page on the phone; a pairing link for a new one.
                let url = format!(
                    "http://localhost:{port}/{}",
                    pair_url_path.as_deref().unwrap_or("")
                );
                let quoted = format!("'{url}'");
                let args = [
                    "-s",
                    &phone.serial,
                    "shell",
                    "am",
                    "start",
                    "-a",
                    "android.intent.action.VIEW",
                    "-d",
                    &quoted,
                ];
                if let Err(e) = run(&adb, &args) {
                    tracing::warn!("opening DeskSpan on {}: {e}", phone.serial);
                }
            }
        }
        let mut g = self.lock();
        let present: HashSet<&String> = phones.iter().map(|p| &p.serial).collect();
        g.opened.retain(|s| present.contains(s));
        g.phones = phones;
    }

    /// A picture connection's TCP port: forward it to every ready phone now.
    pub fn add_port(&self, port: u16) {
        let (adb, phones) = {
            let mut g = self.lock();
            g.ports.insert(port);
            (g.adb.clone(), g.phones.clone())
        };
        let Some(adb) = adb else { return };
        let spec = format!("tcp:{port}");
        for phone in phones.iter().filter(|p| p.state == "ready") {
            if let Err(e) = run(&adb, &["-s", &phone.serial, "reverse", &spec, &spec]) {
                tracing::warn!("adb reverse {port} on {}: {e}", phone.serial);
            }
        }
    }

    pub fn remove_port(&self, port: u16) {
        let (adb, phones) = {
            let mut g = self.lock();
            g.ports.remove(&port);
            (g.adb.clone(), g.phones.clone())
        };
        let Some(adb) = adb else { return };
        let spec = format!("tcp:{port}");
        for phone in phones.iter().filter(|p| p.state == "ready") {
            let _ = run(&adb, &["-s", &phone.serial, "reverse", "--remove", &spec]);
        }
    }
}

/// `adb devices -l` → phones.
pub fn parse_devices(out: &str) -> Vec<UsbPhone> {
    out.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next()?.to_string();
            let state = match words.next()? {
                "device" => "ready",
                "unauthorized" => "allow",
                _ => "offline",
            };
            let model = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("model:"))
                .map(|m| m.replace('_', " "))
                .unwrap_or_else(|| "Android phone".into());
            Some(UsbPhone {
                serial,
                model,
                state,
            })
        })
        .collect()
}

/// Run adb with a time limit; its output, or why not.
fn run(adb: &Path, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(adb)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run adb: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                if status.success() {
                    return Ok(out);
                }
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                return Err(err.trim().to_string());
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(30)),
            Ok(None) => {
                let _ = child.kill();
                return Err("adb didn't answer".into());
            }
            Err(e) => return Err(format!("adb: {e}")),
        }
    }
}

fn exe() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

/// An adb that's already here: our own copy, Android Studio's, Homebrew's,
/// or one on PATH.
fn find_adb(data_dir: &Path) -> Option<PathBuf> {
    let mut places = vec![data_dir.join("platform-tools").join(exe())];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        places.push(home.join("Library/Android/sdk/platform-tools/adb"));
        places.push(home.join("Android/Sdk/platform-tools/adb"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        places.push(local.join("Android/Sdk/platform-tools/adb.exe"));
    }
    places.push(PathBuf::from("/opt/homebrew/bin/adb"));
    places.push(PathBuf::from("/usr/local/bin/adb"));
    if let Some(found) = places.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    // On PATH?
    Command::new(exe())
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|s| s.success())
        .map(|_| PathBuf::from(exe()))
}

/// Google's platform-tools for this system, unpacked into the data folder.
fn download(data_dir: &Path) -> Result<PathBuf, String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "windows",
        "linux" => "linux",
        other => return Err(format!("no Android tools for {other}")),
    };
    let url = format!("https://dl.google.com/android/repository/platform-tools-latest-{os}.zip");
    let zip = tauri::async_runtime::block_on(async {
        let res = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?
            .get(&url)
            .send()
            .await
            .map_err(|e| {
                format!(
                    "cannot download Google's Android tools ({e}); check the internet connection"
                )
            })?;
        if !res.status().is_success() {
            return Err(format!(
                "cannot download Google's Android tools (HTTP {})",
                res.status()
            ));
        }
        res.bytes()
            .await
            .map_err(|e| format!("the download broke off: {e}"))
    })?;
    unpack(&zip, data_dir)?;
    let adb = data_dir.join("platform-tools").join(exe());
    if adb.is_file() {
        Ok(adb)
    } else {
        Err("Google's download had no adb in it".into())
    }
}

/// Unpack `platform-tools/…` from the zip (nothing outside it).
fn unpack(zip: &[u8], data_dir: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip))
        .map_err(|e| format!("the download is damaged: {e}"))?;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("the download is damaged: {e}"))?;
        let Some(rel) = file.enclosed_name() else {
            continue;
        };
        if !rel.starts_with("platform-tools") {
            continue;
        }
        let dest = data_dir.join(&rel);
        if file.is_dir() {
            std::fs::create_dir_all(&dest).map_err(|e| format!("cannot unpack: {e}"))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot unpack: {e}"))?;
        }
        let mut out = std::fs::File::create(&dest).map_err(|e| format!("cannot unpack: {e}"))?;
        std::io::copy(&mut file, &mut out).map_err(|e| format!("cannot unpack: {e}"))?;
        #[cfg(unix)]
        if let Some(mode) = file.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_adb_device_lists() {
        let out = "* daemon started successfully\n\
                   List of devices attached\n\
                   R5CT1234567            device usb:1-1 product:e3qxxx model:SM_S928B device:e3q transport_id:1\n\
                   0123456789ABCDEF       unauthorized usb:1-2 transport_id:2\n\
                   emulator-5554          offline\n\n";
        let phones = parse_devices(out);
        assert_eq!(phones.len(), 3);
        assert_eq!(phones[0].serial, "R5CT1234567");
        assert_eq!(phones[0].state, "ready");
        assert_eq!(phones[0].model, "SM S928B");
        assert_eq!(phones[1].state, "allow");
        assert_eq!(phones[1].model, "Android phone");
        assert_eq!(phones[2].state, "offline");
        assert!(parse_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn unpacks_only_platform_tools() {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("platform-tools/adb", opts).unwrap();
            w.write_all(b"adb").unwrap();
            w.start_file("evil/outside.txt", opts).unwrap();
            w.write_all(b"no").unwrap();
            w.finish().unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        unpack(buf.get_ref(), dir.path()).unwrap();
        assert!(dir.path().join("platform-tools/adb").is_file());
        assert!(!dir.path().join("evil").exists());
    }
}

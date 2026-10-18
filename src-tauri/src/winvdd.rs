//! Windows: extend with the open-source Virtual Display Driver
//! (github.com/VirtualDrivers/Virtual-Display-Driver, MIT, signed by the
//! SignPath Foundation).
//!
//! Install once (administrator): the driver and Nefarius's nefcon (MIT, to
//! create the root device) are downloaded from GitHub, checked against the
//! SHA-256 values below, the driver's signing certificate is trusted and the
//! device installed. After that DeskSpan drives it without admin rights: it
//! writes the wanted size into C:\VirtualDisplayDriver\vdd_settings.xml
//! (the installer lets users change that folder) and tells the driver over
//! its pipe how many virtual monitors to show.
//!
//! First version: one extended device at a time; more devices mirror.

use crate::media::DisplayPlan;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const PIPE: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
const CONFIG_DIR: &str = r"C:\VirtualDisplayDriver";

const DRIVER_URL: &str = "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip";
const DRIVER_SHA256: &str = "e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a";
const NEFCON_URL: &str = "https://github.com/nefarius/nefcon/releases/download/v1.14.0/nefcon_v1.14.0.zip";
const NEFCON_SHA256: &str = "a15557da24a9efca203158de3b43b0eaf982db231f0194031f1ed428bc13e669";

/// The driver is installed and running (its pipe answers).
pub fn installed() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE)
        .is_ok()
}

/// Install the driver: one Windows administrator prompt. Blocking.
pub fn install() -> Result<(), String> {
    if installed() {
        return Ok(());
    }
    let script = std::env::temp_dir().join("deskspan-install-display-driver.ps1");
    std::fs::write(&script, install_script()).map_err(|e| format!("cannot prepare the installer: {e}"))?;
    // Start-Process -Verb RunAs shows the UAC prompt; -Wait for the result.
    let launcher = format!(
        "$p = Start-Process -FilePath powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{}'; exit $p.ExitCode",
        script.display()
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &launcher])
        .status()
        .map_err(|e| format!("cannot start the installer: {e}"))?;
    let _ = std::fs::remove_file(&script);
    if !status.success() {
        return Err(match status.code() {
            Some(2) => "a download didn't match its expected checksum; nothing was installed".into(),
            Some(1223) | None => "the administrator prompt was cancelled".into(),
            Some(c) => format!("the driver installer stopped (code {c})"),
        });
    }
    // The device takes a moment to start.
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if installed() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("the driver was installed but didn't start; restart Windows and try again".into())
}

/// PowerShell run as administrator: download, verify, trust, install.
fn install_script() -> String {
    format!(
        r#"$ErrorActionPreference = 'Stop'
$tmp = Join-Path $env:TEMP ('deskspan-vdd-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {{
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $drv = Join-Path $tmp 'driver.zip'; $nef = Join-Path $tmp 'nefcon.zip'
  Invoke-WebRequest -UseBasicParsing -Uri '{DRIVER_URL}' -OutFile $drv
  Invoke-WebRequest -UseBasicParsing -Uri '{NEFCON_URL}' -OutFile $nef
  if ((Get-FileHash $drv -Algorithm SHA256).Hash -ne '{DRIVER_SHA256}') {{ exit 2 }}
  if ((Get-FileHash $nef -Algorithm SHA256).Hash -ne '{NEFCON_SHA256}') {{ exit 2 }}
  Expand-Archive $drv -DestinationPath $tmp -Force
  Expand-Archive $nef -DestinationPath $tmp -Force
  # Trust the driver's signer (SignPath Foundation) for driver installs.
  $certs = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2Collection
  $certs.Import([IO.File]::ReadAllBytes((Join-Path $tmp 'VirtualDisplayDriver\mttvdd.cat')))
  $store = New-Object System.Security.Cryptography.X509Certificates.X509Store('TrustedPublisher','LocalMachine')
  $store.Open('ReadWrite'); foreach ($c in $certs) {{ $store.Add($c) }}; $store.Close()
  # Settings folder DeskSpan may change without admin: no monitors yet.
  New-Item -ItemType Directory -Path '{CONFIG_DIR}' -Force | Out-Null
  $xml = Get-Content (Join-Path $tmp 'VirtualDisplayDriver\vdd_settings.xml') -Raw
  $xml = $xml -replace '<count>\d+</count>', '<count>0</count>'
  Set-Content -Path '{CONFIG_DIR}\vdd_settings.xml' -Value $xml -Encoding UTF8
  & icacls '{CONFIG_DIR}' /grant '*S-1-5-32-545:(OI)(CI)M' | Out-Null
  $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {{ 'ARM64' }} else {{ 'x64' }}
  # The driver project's own install command (devcon style).
  $inst = Start-Process -FilePath (Join-Path $tmp "$arch\nefconw.exe") -Wait -PassThru -WorkingDirectory $tmp -ArgumentList 'install','.\VirtualDisplayDriver\MttVDD.inf','Root\MttVDD'
  Start-Sleep -Seconds 5
  exit $inst.ExitCode
}} finally {{
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}}
"#
    )
}

/// Send one command to the driver ("SETDISPLAYCOUNT 1", "RELOAD_DRIVER").
fn command(cmd: &str) -> Result<(), String> {
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE)
        .map_err(|e| format!("the display driver isn't running ({e})"))?;
    let wide: Vec<u8> = cmd
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    pipe.write_all(&wide)
        .map_err(|e| format!("the display driver didn't take the command: {e}"))
}

/// Settings file: one monitor offered at `w`×`h` (60 Hz).
fn write_settings(count: u32, w: u32, h: u32) -> Result<(), String> {
    let path = PathBuf::from(CONFIG_DIR).join("vdd_settings.xml");
    let xml = std::fs::read_to_string(&path).map_err(|e| format!("cannot read the driver settings: {e}"))?;
    let xml = replace_between(&xml, "<count>", "</count>", &count.to_string());
    let list = format!(
        "\n        <resolution>\n            <width>{w}</width>\n            <height>{h}</height>\n            <refresh_rate>60</refresh_rate>\n        </resolution>\n    "
    );
    let xml = replace_between(&xml, "<resolutions>", "</resolutions>", &list);
    std::fs::write(&path, xml).map_err(|e| format!("cannot change the driver settings: {e}"))
}

/// `text` with whatever is between the first `open`…`close` replaced.
fn replace_between(text: &str, open: &str, close: &str, inner: &str) -> String {
    match (text.find(open), text.find(close)) {
        (Some(a), Some(b)) if a + open.len() <= b => {
            format!("{}{}{}", &text[..a + open.len()], inner, &text[b..])
        }
        _ => text.to_string(),
    }
}

/// Show one virtual monitor sized for `plan`; its xcap monitor id.
pub fn create(plan: DisplayPlan) -> Result<u32, String> {
    if !installed() {
        return Err("the Windows display driver isn't installed (DeskSpan › Share screen › Extend on Windows)".into());
    }
    let before: HashSet<u32> = monitor_ids();
    // Windows has no Retina-style backing here: the desktop gets the
    // plan's point size, the phone scales it up.
    let (w, h) = (plan.points_w, plan.points_h);
    write_settings(1, w, h)?;
    command("SETDISPLAYCOUNT 1")?;
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(300));
        let now = monitor_ids();
        if let Some(id) = now.difference(&before).next() {
            tracing::info!("virtual monitor {id}: {w}x{h}");
            return Ok(*id);
        }
    }
    Err("Windows didn't add the extra monitor".into())
}

/// Take the virtual monitor away again.
pub fn remove() {
    if let Err(e) = command("SETDISPLAYCOUNT 0") {
        tracing::warn!("removing the virtual monitor: {e}");
    }
}

fn monitor_ids() -> HashSet<u32> {
    xcap::Monitor::all()
        .map(|all| all.iter().filter_map(|m| m.id().ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_rewritten_in_place() {
        let xml = "<vdd_settings><monitors><count>0</count></monitors><resolutions><resolution><width>1920</width></resolution></resolutions></vdd_settings>";
        let out = replace_between(xml, "<count>", "</count>", "1");
        assert!(out.contains("<count>1</count>"));
        let out = replace_between(&out, "<resolutions>", "</resolutions>", "X");
        assert!(out.contains("<resolutions>X</resolutions>"));
        assert_eq!(replace_between("abc", "<x>", "</x>", "y"), "abc");
    }

    #[test]
    fn installer_pins_both_downloads() {
        let s = install_script();
        assert!(s.contains(DRIVER_SHA256) && s.contains(NEFCON_SHA256));
        assert!(s.contains("Root\\MttVDD"));
    }
}

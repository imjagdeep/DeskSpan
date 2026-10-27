//! Every screen on the computer, like the system's display settings: real
//! monitors (HDMI, DisplayPort, USB-C, VGA…), the built-in screen and
//! DeskSpan's own extra displays. Arrange them, choose the main one, and
//! switch between extending and duplicating. A watcher notices monitors
//! being plugged in so the app can ask what to do with them.
//!
//! Windows: the display configuration API (what Settings › Display uses).
//! macOS: Quartz display configuration. Linux: listed only.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Screen {
    pub id: String,
    pub name: String,
    /// "HDMI", "DisplayPort", "USB-C", "VGA", "DVI", "Built-in", "Wireless",
    /// "DeskSpan", "Virtual", "External"
    pub connection: String,
    /// Position on the desktop and size, in the system's desktop units.
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub main: bool,
    /// Showing a copy of another screen (its id).
    pub mirror_of: Option<String>,
    /// One of DeskSpan's extra displays (a phone or tablet).
    pub deskspan: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Layout {
    pub screens: Vec<Screen>,
    /// Positions and the main screen can be changed here.
    pub can_arrange: bool,
    /// Extend / duplicate can be switched here.
    pub can_mirror: bool,
    /// "extend" or "duplicate".
    pub mode: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Place {
    pub id: String,
    pub x: i32,
    pub y: i32,
}

/// Move everything so `main` is at 0,0 (both systems put the main screen
/// there), keeping the arrangement.
pub fn anchor(places: &mut [Place], main: &str) -> Result<(), String> {
    let (mx, my) = places
        .iter()
        .find(|p| p.id == main)
        .map(|p| (p.x, p.y))
        .ok_or("that screen isn't connected any more")?;
    for p in places.iter_mut() {
        p.x -= mx;
        p.y -= my;
    }
    Ok(())
}

fn label_for_name(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_lowercase();
    if n.starts_with("deskspan") || n.contains("virtual display") || n.contains("vdd") {
        Some("DeskSpan")
    } else {
        None
    }
}

// ---------- Windows ----------

#[cfg(windows)]
mod sys {
    use super::{Layout, Place, Screen};
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Foundation::ERROR_SUCCESS;

    struct Config {
        paths: Vec<DISPLAYCONFIG_PATH_INFO>,
        modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    }

    fn query() -> Result<Config, String> {
        for _ in 0..3 {
            let (mut np, mut nm) = (0u32, 0u32);
            if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) }
                != ERROR_SUCCESS
            {
                return Err("Windows didn't report the displays".into());
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let r = unsafe {
                QueryDisplayConfig(
                    QDC_ONLY_ACTIVE_PATHS,
                    &mut np,
                    paths.as_mut_ptr(),
                    &mut nm,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            if r == ERROR_SUCCESS {
                paths.truncate(np as usize);
                modes.truncate(nm as usize);
                return Ok(Config { paths, modes });
            }
            // ERROR_INSUFFICIENT_BUFFER: a display came or went; ask again.
        }
        Err("the displays kept changing; try again".into())
    }

    fn id_of(p: &DISPLAYCONFIG_PATH_INFO) -> String {
        let a = p.targetInfo.adapterId;
        format!("{:x}-{:x}-{}", a.HighPart, a.LowPart, p.targetInfo.id)
    }

    fn name_of(p: &DISPLAYCONFIG_PATH_INFO) -> (String, DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY) {
        let mut n = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        n.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        n.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        n.header.adapterId = p.targetInfo.adapterId;
        n.header.id = p.targetInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut n.header) } != 0 {
            return (String::new(), p.targetInfo.outputTechnology);
        }
        let end = n
            .monitorFriendlyDeviceName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(64);
        (
            String::from_utf16_lossy(&n.monitorFriendlyDeviceName[..end]),
            n.outputTechnology,
        )
    }

    fn connection(t: DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY) -> &'static str {
        match t {
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI => "HDMI",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL => "DisplayPort",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_USB_TUNNEL => "USB-C",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HD15 => "VGA",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DVI => "DVI",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL
            | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED
            | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED
            | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_LVDS => "Built-in",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_MIRACAST => "Wireless",
            DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL
            | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED => "Virtual",
            _ => "External",
        }
    }

    fn source_index(p: &DISPLAYCONFIG_PATH_INFO) -> usize {
        unsafe { p.sourceInfo.Anonymous.modeInfoIdx as usize }
    }

    pub fn layout() -> Result<Layout, String> {
        let c = query()?;
        let mut screens = Vec::new();
        // Several targets on one source = duplicated.
        let mut first_on_source: Vec<(u32, String)> = Vec::new();
        for p in &c.paths {
            let Some(m) = c.modes.get(source_index(p)) else {
                continue;
            };
            if m.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                continue;
            }
            let sm = unsafe { m.Anonymous.sourceMode };
            let (name, tech) = name_of(p);
            let id = id_of(p);
            let key = p.sourceInfo.id;
            let mirror_of = first_on_source
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, id)| id.clone());
            if mirror_of.is_none() {
                first_on_source.push((key, id.clone()));
            }
            let deskspan = super::label_for_name(&name).is_some();
            screens.push(Screen {
                connection: if deskspan {
                    "DeskSpan".into()
                } else {
                    connection(tech).into()
                },
                name: if name.is_empty() {
                    "Display".into()
                } else {
                    name
                },
                x: sm.position.x,
                y: sm.position.y,
                w: sm.width,
                h: sm.height,
                main: sm.position.x == 0 && sm.position.y == 0,
                mirror_of,
                deskspan,
                id,
            });
        }
        let duplicated = screens.iter().any(|s| s.mirror_of.is_some());
        Ok(Layout {
            screens,
            can_arrange: true,
            can_mirror: true,
            mode: if duplicated { "duplicate" } else { "extend" }.into(),
        })
    }

    pub fn arrange(places: &[Place]) -> Result<(), String> {
        let mut c = query()?;
        for p in c.paths.clone() {
            let id = id_of(&p);
            let Some(place) = places.iter().find(|x| x.id == id) else {
                continue;
            };
            let i = source_index(&p);
            if let Some(m) = c.modes.get_mut(i) {
                if m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                    unsafe {
                        m.Anonymous.sourceMode.position.x = place.x;
                        m.Anonymous.sourceMode.position.y = place.y;
                    }
                }
            }
        }
        // ALLOW_CHANGES: Windows closes small gaps and overlaps itself.
        let r = unsafe {
            SetDisplayConfig(
                Some(&c.paths),
                Some(&c.modes),
                SDC_APPLY
                    | SDC_USE_SUPPLIED_DISPLAY_CONFIG
                    | SDC_SAVE_TO_DATABASE
                    | SDC_ALLOW_CHANGES,
            )
        };
        if r != 0 {
            return Err(format!(
                "Windows didn't accept that arrangement (error {r})"
            ));
        }
        Ok(())
    }

    pub fn set_mode(duplicate: bool) -> Result<(), String> {
        let topology = if duplicate {
            SDC_TOPOLOGY_CLONE
        } else {
            SDC_TOPOLOGY_EXTEND
        };
        let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | topology) };
        if r != 0 {
            return Err(format!("Windows couldn't switch the displays (error {r})"));
        }
        Ok(())
    }
}

// ---------- macOS ----------

#[cfg(target_os = "macos")]
mod sys {
    use super::{Layout, Place, Screen};
    use objc2_core_graphics::*;

    fn ids() -> Result<Vec<u32>, String> {
        let mut list = [0u32; 32];
        let mut n = 0u32;
        let e = unsafe { CGGetActiveDisplayList(32, list.as_mut_ptr(), &mut n) };
        if e != CGError::Success {
            return Err(format!("macOS didn't list the displays ({})", e.0));
        }
        Ok(list[..n as usize].to_vec())
    }

    pub fn layout() -> Result<Layout, String> {
        let names: Vec<(u32, String)> = xcap::Monitor::all()
            .map(|all| {
                all.iter()
                    .filter_map(|m| Some((m.id().ok()?, m.name().ok()?)))
                    .collect()
            })
            .unwrap_or_default();
        let mut screens = Vec::new();
        for id in ids()? {
            let b = CGDisplayBounds(id);
            let name = names
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            let deskspan = super::label_for_name(&name).is_some();
            let mirror = CGDisplayMirrorsDisplay(id);
            screens.push(Screen {
                id: id.to_string(),
                connection: if deskspan {
                    "DeskSpan"
                } else if CGDisplayIsBuiltin(id) {
                    "Built-in"
                } else {
                    "External"
                }
                .into(),
                name: if name.is_empty() {
                    "Display".into()
                } else {
                    name
                },
                x: b.origin.x.round() as i32,
                y: b.origin.y.round() as i32,
                w: b.size.width.round() as u32,
                h: b.size.height.round() as u32,
                main: CGDisplayIsMain(id),
                mirror_of: (mirror != kCGNullDirectDisplay).then(|| mirror.to_string()),
                deskspan,
            });
        }
        let duplicated = screens.iter().any(|s| s.mirror_of.is_some());
        Ok(Layout {
            screens,
            can_arrange: true,
            can_mirror: true,
            mode: if duplicated { "duplicate" } else { "extend" }.into(),
        })
    }

    /// One configuration transaction: `change` adds the changes.
    fn configure(
        change: impl FnOnce(CGDisplayConfigRef) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut cfg: CGDisplayConfigRef = std::ptr::null_mut();
        if unsafe { CGBeginDisplayConfiguration(&mut cfg) } != CGError::Success {
            return Err("macOS didn't allow changing the displays".into());
        }
        if let Err(e) = change(cfg) {
            unsafe { CGCancelDisplayConfiguration(cfg) };
            return Err(e);
        }
        let e = unsafe { CGCompleteDisplayConfiguration(cfg, CGConfigureOption::Permanently) };
        if e != CGError::Success {
            return Err(format!("macOS didn't accept the change ({})", e.0));
        }
        Ok(())
    }

    pub fn arrange(places: &[Place]) -> Result<(), String> {
        configure(|cfg| {
            for p in places {
                let id: u32 = p.id.parse().map_err(|_| "unknown display")?;
                let e = unsafe { CGConfigureDisplayOrigin(cfg, id, p.x, p.y) };
                if e != CGError::Success {
                    return Err(format!("couldn't move a display ({})", e.0));
                }
            }
            Ok(())
        })
    }

    pub fn set_mode(duplicate: bool) -> Result<(), String> {
        let all = ids()?;
        let main = all
            .iter()
            .copied()
            .find(|&d| CGDisplayIsMain(d))
            .ok_or("no main display")?;
        configure(|cfg| {
            for d in all.into_iter().filter(|&d| d != main) {
                let of = if duplicate {
                    main
                } else {
                    kCGNullDirectDisplay
                };
                let e = unsafe { CGConfigureDisplayMirrorOfDisplay(cfg, d, of) };
                if e != CGError::Success {
                    return Err(format!("couldn't change a display ({})", e.0));
                }
            }
            Ok(())
        })
    }
}

// ---------- Linux: listed only ----------

#[cfg(not(any(windows, target_os = "macos")))]
mod sys {
    use super::{Layout, Place, Screen};

    pub fn layout() -> Result<Layout, String> {
        let all = xcap::Monitor::all().map_err(|e| format!("cannot list the screens: {e}"))?;
        let screens = all
            .iter()
            .filter_map(|m| {
                let name = m.name().unwrap_or_default();
                let deskspan = super::label_for_name(&name).is_some();
                Some(Screen {
                    id: m.id().ok()?.to_string(),
                    connection: if deskspan { "DeskSpan" } else { "External" }.into(),
                    name,
                    x: m.x().ok()?,
                    y: m.y().ok()?,
                    w: m.width().ok()?,
                    h: m.height().ok()?,
                    main: m.is_primary().unwrap_or(false),
                    mirror_of: None,
                    deskspan,
                })
            })
            .collect();
        Ok(Layout {
            screens,
            can_arrange: false,
            can_mirror: false,
            mode: "extend".into(),
        })
    }

    pub fn arrange(_places: &[Place]) -> Result<(), String> {
        Err(
            "Arranging displays isn't available on Linux yet; use your desktop's display settings"
                .into(),
        )
    }

    pub fn set_mode(_duplicate: bool) -> Result<(), String> {
        Err(
            "Switching displays isn't available on Linux yet; use your desktop's display settings"
                .into(),
        )
    }
}

pub fn layout() -> Result<Layout, String> {
    sys::layout()
}

/// New positions; the main screen stays where the user put it.
pub fn arrange(mut places: Vec<Place>) -> Result<(), String> {
    let now = layout()?;
    let main = now
        .screens
        .iter()
        .find(|s| s.main)
        .map(|s| s.id.clone())
        .ok_or("no main display")?;
    anchor(&mut places, &main)?;
    sys::arrange(&places)
}

/// Make `id` the main screen (moves the arrangement so it's at 0,0).
pub fn set_main(id: &str) -> Result<(), String> {
    let mut places: Vec<Place> = layout()?
        .screens
        .into_iter()
        .filter(|s| s.mirror_of.is_none())
        .map(|s| Place {
            id: s.id,
            x: s.x,
            y: s.y,
        })
        .collect();
    anchor(&mut places, id)?;
    sys::arrange(&places)
}

pub fn set_mode(duplicate: bool) -> Result<(), String> {
    sys::set_mode(duplicate)
}

#[derive(Clone, Serialize)]
struct Added<'a> {
    screen: &'a Screen,
}

/// Watch for monitors being plugged in (not DeskSpan's own displays):
/// "screens-changed" on any change, "screen-added" for a new monitor, and a
/// notification so it's seen with the window closed.
pub fn watch(app: AppHandle) {
    std::thread::Builder::new()
        .name("deskspan-screens".into())
        .spawn(move || {
            let mut known: Option<(HashSet<String>, Vec<(String, i32, i32, u32, u32)>)> = None;
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let Ok(l) = layout() else { continue };
                let ids: HashSet<String> = l.screens.iter().map(|s| s.id.clone()).collect();
                let shape: Vec<_> = l
                    .screens
                    .iter()
                    .map(|s| (s.id.clone(), s.x, s.y, s.w, s.h))
                    .collect();
                if let Some((old_ids, old_shape)) = &known {
                    if *old_shape != shape {
                        let _ = app.emit("screens-changed", ());
                    }
                    for s in l.screens.iter().filter(|s| {
                        !old_ids.contains(&s.id) && !s.deskspan && s.connection != "Virtual"
                    }) {
                        let _ = app.emit("screen-added", Added { screen: s });
                        crate::notify(
                            &app,
                            &format!("Display connected: {}", s.name),
                            &format!(
                                "{} · {}×{}. Open DeskSpan › Displays to extend or duplicate it.",
                                s.connection, s.w, s.h
                            ),
                        );
                    }
                }
                known = Some((ids, shape));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: &str, x: i32, y: i32) -> Place {
        Place {
            id: id.into(),
            x,
            y,
        }
    }

    #[test]
    fn anchoring_moves_the_main_screen_to_the_origin() {
        let mut v = vec![p("a", 100, 50), p("b", 2020, 50)];
        anchor(&mut v, "a").unwrap();
        assert_eq!((v[0].x, v[0].y, v[1].x, v[1].y), (0, 0, 1920, 0));
        // A screen to the left of the main one ends up with negative x.
        let mut v = vec![p("a", 0, 0), p("b", 1920, 0)];
        anchor(&mut v, "b").unwrap();
        assert_eq!((v[0].x, v[1].x), (-1920, 0));
        assert!(anchor(&mut v, "gone").is_err());
    }

    #[test]
    fn deskspan_displays_are_recognised_by_name() {
        assert_eq!(label_for_name("DeskSpan – iPad"), Some("DeskSpan"));
        assert_eq!(label_for_name("VDD by MTT"), Some("DeskSpan"));
        assert_eq!(label_for_name("DELL U2720Q"), None);
    }
}

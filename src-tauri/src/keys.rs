//! Keys from a viewer, by their physical position (the browser's
//! `KeyboardEvent.code`, the same on every keyboard layout), and what each
//! one is on Windows (virtual-key code) and macOS (virtual key code).
//! The computer being controlled types with its own layout.

/// One key.
// Each OS reads only its own fields (win/ext on Windows, mac/modifier on
// macOS), so the others look unused there.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDef {
    pub code: &'static str,
    /// Windows virtual-key code.
    pub win: u16,
    /// Windows: an "extended" key (arrows, right-hand modifiers…).
    pub ext: bool,
    /// macOS virtual key code (`NONE`: no such key, ignored).
    pub mac: u16,
    pub modifier: Option<Modifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    /// ⌘ on a Mac, the Windows key on a PC.
    Meta,
}

pub const NONE: u16 = 0xFFFF;

const fn k(code: &'static str, win: u16, ext: bool, mac: u16) -> KeyDef {
    KeyDef {
        code,
        win,
        ext,
        mac,
        modifier: None,
    }
}

const fn m(code: &'static str, win: u16, ext: bool, mac: u16, modifier: Modifier) -> KeyDef {
    KeyDef {
        code,
        win,
        ext,
        mac,
        modifier: Some(modifier),
    }
}

pub const KEYS: &[KeyDef] = &[
    k("KeyA", 0x41, false, 0x00),
    k("KeyB", 0x42, false, 0x0B),
    k("KeyC", 0x43, false, 0x08),
    k("KeyD", 0x44, false, 0x02),
    k("KeyE", 0x45, false, 0x0E),
    k("KeyF", 0x46, false, 0x03),
    k("KeyG", 0x47, false, 0x05),
    k("KeyH", 0x48, false, 0x04),
    k("KeyI", 0x49, false, 0x22),
    k("KeyJ", 0x4A, false, 0x26),
    k("KeyK", 0x4B, false, 0x28),
    k("KeyL", 0x4C, false, 0x25),
    k("KeyM", 0x4D, false, 0x2E),
    k("KeyN", 0x4E, false, 0x2D),
    k("KeyO", 0x4F, false, 0x1F),
    k("KeyP", 0x50, false, 0x23),
    k("KeyQ", 0x51, false, 0x0C),
    k("KeyR", 0x52, false, 0x0F),
    k("KeyS", 0x53, false, 0x01),
    k("KeyT", 0x54, false, 0x11),
    k("KeyU", 0x55, false, 0x20),
    k("KeyV", 0x56, false, 0x09),
    k("KeyW", 0x57, false, 0x0D),
    k("KeyX", 0x58, false, 0x07),
    k("KeyY", 0x59, false, 0x10),
    k("KeyZ", 0x5A, false, 0x06),
    k("Digit1", 0x31, false, 0x12),
    k("Digit2", 0x32, false, 0x13),
    k("Digit3", 0x33, false, 0x14),
    k("Digit4", 0x34, false, 0x15),
    k("Digit5", 0x35, false, 0x17),
    k("Digit6", 0x36, false, 0x16),
    k("Digit7", 0x37, false, 0x1A),
    k("Digit8", 0x38, false, 0x1C),
    k("Digit9", 0x39, false, 0x19),
    k("Digit0", 0x30, false, 0x1D),
    k("Enter", 0x0D, false, 0x24),
    k("Escape", 0x1B, false, 0x35),
    k("Backspace", 0x08, false, 0x33),
    k("Tab", 0x09, false, 0x30),
    k("Space", 0x20, false, 0x31),
    k("Minus", 0xBD, false, 0x1B),
    k("Equal", 0xBB, false, 0x18),
    k("BracketLeft", 0xDB, false, 0x21),
    k("BracketRight", 0xDD, false, 0x1E),
    k("Backslash", 0xDC, false, 0x2A),
    k("IntlBackslash", 0xE2, false, 0x0A),
    k("Semicolon", 0xBA, false, 0x29),
    k("Quote", 0xDE, false, 0x27),
    k("Backquote", 0xC0, false, 0x32),
    k("Comma", 0xBC, false, 0x2B),
    k("Period", 0xBE, false, 0x2F),
    k("Slash", 0xBF, false, 0x2C),
    k("CapsLock", 0x14, false, 0x39),
    k("F1", 0x70, false, 0x7A),
    k("F2", 0x71, false, 0x78),
    k("F3", 0x72, false, 0x63),
    k("F4", 0x73, false, 0x76),
    k("F5", 0x74, false, 0x60),
    k("F6", 0x75, false, 0x61),
    k("F7", 0x76, false, 0x62),
    k("F8", 0x77, false, 0x64),
    k("F9", 0x78, false, 0x65),
    k("F10", 0x79, false, 0x6D),
    k("F11", 0x7A, false, 0x67),
    k("F12", 0x7B, false, 0x6F),
    // No such keys on a Mac: the nearest ones.
    k("PrintScreen", 0x2C, true, 0x69),
    k("ScrollLock", 0x91, false, 0x6B),
    k("Pause", 0x13, false, 0x71),
    k("Insert", 0x2D, true, 0x72),
    k("Delete", 0x2E, true, 0x75),
    k("Home", 0x24, true, 0x73),
    k("End", 0x23, true, 0x77),
    k("PageUp", 0x21, true, 0x74),
    k("PageDown", 0x22, true, 0x79),
    k("ArrowLeft", 0x25, true, 0x7B),
    k("ArrowRight", 0x27, true, 0x7C),
    k("ArrowDown", 0x28, true, 0x7D),
    k("ArrowUp", 0x26, true, 0x7E),
    k("ContextMenu", 0x5D, true, 0x6E),
    k("NumLock", 0x90, true, 0x47),
    k("Numpad0", 0x60, false, 0x52),
    k("Numpad1", 0x61, false, 0x53),
    k("Numpad2", 0x62, false, 0x54),
    k("Numpad3", 0x63, false, 0x55),
    k("Numpad4", 0x64, false, 0x56),
    k("Numpad5", 0x65, false, 0x57),
    k("Numpad6", 0x66, false, 0x58),
    k("Numpad7", 0x67, false, 0x59),
    k("Numpad8", 0x68, false, 0x5B),
    k("Numpad9", 0x69, false, 0x5C),
    k("NumpadMultiply", 0x6A, false, 0x43),
    k("NumpadAdd", 0x6B, false, 0x45),
    k("NumpadSubtract", 0x6D, false, 0x4E),
    k("NumpadDecimal", 0x6E, false, 0x41),
    k("NumpadDivide", 0x6F, true, 0x4B),
    k("NumpadEnter", 0x0D, true, 0x4C),
    k("NumpadEqual", NONE, false, 0x51),
    m("ShiftLeft", 0xA0, false, 0x38, Modifier::Shift),
    m("ShiftRight", 0xA1, false, 0x3C, Modifier::Shift),
    m("ControlLeft", 0xA2, false, 0x3B, Modifier::Control),
    m("ControlRight", 0xA3, true, 0x3E, Modifier::Control),
    m("AltLeft", 0xA4, false, 0x3A, Modifier::Alt),
    m("AltRight", 0xA5, true, 0x3D, Modifier::Alt),
    m("MetaLeft", 0x5B, true, 0x37, Modifier::Meta),
    m("MetaRight", 0x5C, true, 0x36, Modifier::Meta),
];

/// The key for a `KeyboardEvent.code`. With `swap` (a Mac keyboard
/// controlling a PC, or the other way round), ⌘ and Ctrl trade places so
/// ⌘C copies on Windows and Ctrl+C copies on the Mac.
pub fn find(code: &str, swap: bool) -> Option<KeyDef> {
    let code = match (swap, code) {
        (true, "MetaLeft") => "ControlLeft",
        (true, "MetaRight") => "ControlRight",
        (true, "ControlLeft") => "MetaLeft",
        (true, "ControlRight") => "MetaRight",
        // Older browsers name the ⌘ / Windows keys this way.
        (true, "OSLeft") => "ControlLeft",
        (true, "OSRight") => "ControlRight",
        (false, "OSLeft") => "MetaLeft",
        (false, "OSRight") => "MetaRight",
        (_, c) => c,
    };
    KEYS.iter().copied().find(|d| d.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        for (i, a) in KEYS.iter().enumerate() {
            assert!(
                KEYS[i + 1..].iter().all(|b| b.code != a.code),
                "{} twice",
                a.code
            );
        }
    }

    #[test]
    fn command_and_control_swap_between_mac_and_pc() {
        assert_eq!(find("KeyC", true).unwrap().win, 0x43);
        assert_eq!(find("MetaLeft", true).unwrap().code, "ControlLeft");
        assert_eq!(find("ControlLeft", true).unwrap().code, "MetaLeft");
        assert_eq!(find("MetaLeft", false).unwrap().code, "MetaLeft");
        assert_eq!(find("OSLeft", false).unwrap().code, "MetaLeft");
        assert!(find("Fn", false).is_none());
        assert!(find("", false).is_none());
    }
}

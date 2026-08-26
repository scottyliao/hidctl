//! Names for the HID usage pages and usages that show up on a typical PC.
//!
//! The HID Usage Tables spec defines thousands of usage page/usage
//! combinations (most of it niche — VR controllers, medical instruments,
//! braille displays). We only bother naming the ones actually likely to show
//! up when enumerating a normal Windows PC or laptop, which [`crate::main`]
//! prints next to the raw `page:usage` hex pair via [`describe`]. Anything
//! not in these tables still gets displayed as plain hex — this module only
//! ever adds a friendly label on top, never replaces the numbers.

/// Human-readable name for a usage page, if it is one we know.
///
/// `0xFF00..=0xFFFF` is not one specific vendor's page — the HID spec
/// reserves that whole block for vendor-defined pages, so every value in it
/// maps to the same generic "Vendor-defined" label (as seen throughout this
/// project's own enumeration output, e.g. `FF31`, `FFC0`, `FFC1` for
/// ASUS-specific vendor channels).
pub fn page_name(page: u16) -> Option<&'static str> {
    Some(match page {
        0x01 => "Generic Desktop",
        0x02 => "Simulation Controls",
        0x03 => "VR Controls",
        0x04 => "Sport Controls",
        0x05 => "Game Controls",
        0x06 => "Generic Device Controls",
        0x07 => "Keyboard/Keypad",
        0x08 => "LED",
        0x09 => "Button",
        0x0A => "Ordinal",
        0x0B => "Telephony Device",
        0x0C => "Consumer",
        0x0D => "Digitizer",
        0x0E => "Haptics",
        0x0F => "Physical Input Device",
        0x10 => "Unicode",
        0x11 => "SoC",
        0x12 => "Eye and Head Tracker",
        0x14 => "Auxiliary Display",
        0x20 => "Sensor",
        0x40 => "Medical Instrument",
        0x41 => "Braille Display",
        0x59 => "Lighting and Illumination",
        0x84 => "Power Device",
        0x85 => "Battery System",
        0x8C => "Bar Code Scanner",
        0x8D => "Scale",
        0x8E => "Magnetic Stripe Reader",
        0x90 => "Camera Control",
        0x91 => "Arcade",
        0x92 => "Gaming Device",
        0xF1D0 => "FIDO Alliance",
        0xFF00..=0xFFFF => "Vendor-defined",
        // Anything else: no page name, so `describe` falls all the way
        // through to showing raw hex only.
        _ => return None,
    })
}

/// Human-readable name for a usage within its page, if it is one we know.
///
/// Usage IDs are only meaningful relative to their page (e.g. usage `0x01`
/// is "Pointer" on the Generic Desktop page but "Phone" would be a different
/// number entirely on the Telephony page), so this is keyed on the
/// `(page, usage)` pair rather than `usage` alone.
pub fn usage_name(page: u16, usage: u16) -> Option<&'static str> {
    Some(match (page, usage) {
        // Generic Desktop (0x01): the page carrying the "what kind of input
        // device is this" usages — Mouse/Keyboard/Gamepad/Joystick and so on.
        (0x01, 0x01) => "Pointer",
        (0x01, 0x02) => "Mouse",
        (0x01, 0x04) => "Joystick",
        (0x01, 0x05) => "Gamepad",
        (0x01, 0x06) => "Keyboard",
        (0x01, 0x07) => "Keypad",
        (0x01, 0x08) => "Multi-axis Controller",
        (0x01, 0x09) => "Tablet PC System Controls",
        (0x01, 0x0A) => "Water Cooling Device",
        (0x01, 0x0B) => "Computer Chassis Device",
        (0x01, 0x0C) => "Wireless Radio Controls",
        (0x01, 0x0D) => "Portable Device Control",
        (0x01, 0x0E) => "System Multi-Axis Controller",
        (0x01, 0x0F) => "Spatial Controller",
        (0x01, 0x80) => "System Control",
        // Telephony Device (0x0B).
        (0x0B, 0x01) => "Phone",
        (0x0B, 0x05) => "Headset",
        // Consumer (0x0C): media/volume/power keys ("consumer control"
        // devices in Device Manager terms).
        (0x0C, 0x01) => "Consumer Control",
        // Digitizer (0x0D): touchpads, touchscreens, pens.
        (0x0D, 0x01) => "Digitizer",
        (0x0D, 0x02) => "Pen",
        (0x0D, 0x04) => "Touch Screen",
        (0x0D, 0x05) => "Touch Pad",
        (0x0D, 0x0E) => "Device Configuration",
        // Eye and Head Tracker (0x12).
        (0x12, 0x01) => "Eye Tracker",
        (0x12, 0x02) => "Head Tracker",
        // FIDO Alliance (0xF1D0): security keys / U2F authenticators.
        (0xF1D0, 0x01) => "U2F Authenticator Device",
        // Anything else: page name alone (if any) is still shown by
        // `describe`; only the usage-specific label is missing here.
        _ => return None,
    })
}

/// `"Generic Desktop / Keyboard"`, degrading to just the page or to nothing.
///
/// Three possible outcomes, most to least informative:
/// - both page and usage are known → `"<page> / <usage>"`
/// - only the page is known (usage is some value we haven't named) →
///   just `"<page>"`
/// - neither is known → `None`, and the caller ([`crate::main::print_device`])
///   shows the raw `page:usage` hex with no parenthetical at all.
pub fn describe(page: u16, usage: u16) -> Option<String> {
    match (page_name(page), usage_name(page, usage)) {
        (Some(page), Some(usage)) => Some(format!("{page} / {usage}")),
        (Some(page), None) => Some(page.to_string()),
        _ => None,
    }
}

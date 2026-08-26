//! Lists the HID devices present on the system, grouped by physical device.
//!
//! Everything Windows-specific lives in [`win32`] (raw FFI) and [`hid`] (safe
//! wrappers) — this file is just the CLI: parse arguments, call
//! [`hid::enumerate`], optionally filter by vendor and/or product ID, sort,
//! group interfaces back into the physical devices they belong to, and print.
//!
//! By default each device gets a two-line summary (name plus interface
//! count); `--detail` switches to a full per-interface dump instead.

mod hid;
mod usage;
mod win32;

use std::process::ExitCode;

use hid::HidDevice;

/// ASUSTek Computer Inc.'s USB vendor ID. Lets `asus` work as a shorthand for
/// `--vid 0x0B05` without having to remember/type the value.
const ASUS_VID: u16 = 0x0B05;

const USAGE: &str = "usage: hidctl [--vid <id>] [--pid <id>] [asus] [--detail]\n\
                     ids are decimal unless prefixed with 0x, e.g. 2821 or 0x0B05";

/// Parsed command line.
struct Args {
    /// Only list devices from this vendor ID; `None` matches any vendor.
    vid: Option<u16>,
    /// Only list devices with this product ID; `None` matches any product.
    /// Usable on its own, though a PID is only meaningful per vendor, so it
    /// is normally paired with `vid`.
    pid: Option<u16>,
    /// Print the full per-interface dump instead of the grouped summary.
    detail: bool,
}

impl Args {
    /// Whether any device filter was requested, which decides both whether
    /// the retain pass below runs and whether the counts are reported as
    /// "matched" rather than "present".
    fn has_filter(&self) -> bool {
        self.vid.is_some() || self.pid.is_some()
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("hidctl: {msg}");
            return ExitCode::FAILURE;
        }
    };

    let mut devices = match hid::enumerate() {
        Ok(devices) => devices,
        Err(err) => {
            eprintln!("hidctl: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.has_filter() {
        // `vendor_product_id()` also recovers VID/PID from the instance ID
        // for devices we couldn't open (see hid.rs), so devices Windows
        // locked (e.g. the OS's primary mouse/keyboard collections) still
        // get matched instead of silently vanishing from a filter.
        devices.retain(|device| match device.vendor_product_id() {
            Some((vid, pid)) => {
                args.vid.is_none_or(|want| want == vid)
                    && args.pid.is_none_or(|want| want == pid)
            }
            // With no recoverable VID/PID there is no way to tell whether
            // this interface matches, so an explicit filter drops it.
            None => false,
        });
    }

    // Sort by (VID, PID, usage page, usage) so every interface belonging to
    // the same physical device ends up contiguous — which is what lets
    // `group_by_product` below collapse them by simply walking the list —
    // then break ties on `path` for full determinism between runs.
    devices.sort_by(|a, b| {
        let key = |d: &HidDevice| d.vendor_product_id().map(|(vid, pid)| {
            // Devices we couldn't open have no usage page/usage (that comes
            // from HidP_GetCaps, which needs an open handle) — default to
            // (0, 0) so they still sort next to their siblings by VID/PID
            // alone rather than being excluded from the ordering.
            let usage = d.info.as_ref().map(|i| (i.usage_page, i.usage)).unwrap_or_default();
            (vid, pid, usage)
        });
        key(a).cmp(&key(b)).then_with(|| a.path.cmp(&b.path))
    });

    if devices.is_empty() {
        println!("No matching HID devices present.");
        return ExitCode::SUCCESS;
    }

    let groups = group_by_product(&devices);

    // "present" for the full unfiltered listing, "matched" once any filter is
    // in play, so the counts don't misleadingly imply they're the system-wide
    // totals.
    let verb = if args.has_filter() { "matched" } else { "present" };
    println!(
        "{} device(s), {} interface(s) {verb}.",
        groups.len(),
        devices.len()
    );
    let unreadable = devices.iter().filter(|d| d.info.is_none()).count();
    if unreadable > 0 {
        println!("{unreadable} interface(s) could not be opened for querying.");
    }
    println!();

    if args.detail {
        for (index, device) in devices.iter().enumerate() {
            if index > 0 {
                println!();
            }
            print_device(index + 1, device);
        }
    } else {
        for (key, interfaces) in &groups {
            // With a vendor filter the VID is the same on every line and just
            // adds noise, so only the PID is shown. Without one — including
            // when only --pid was given, which can still match across
            // vendors — the full VID:PID pair is needed to tell them apart.
            print_group(*key, interfaces, args.vid.is_some());
        }
    }

    ExitCode::SUCCESS
}

/// Parses the command line into [`Args`].
///
/// Deliberately minimal hand-rolled parsing (no argument-parsing crate) to
/// match the rest of the project's no-dependencies approach.
fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        vid: None,
        pid: None,
        detail: false,
    };
    let mut rest = std::env::args().skip(1);

    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--detail" | "-d" => args.detail = true,
            // ID given as a separate argument: `--vid 0x0B05`.
            "--vid" => args.vid = Some(parse_id(&take_value(&arg, &mut rest)?, "vendor")?),
            "--pid" => args.pid = Some(parse_id(&take_value(&arg, &mut rest)?, "product")?),
            // ID given inline: `--vid=0x0B05`.
            other if other.starts_with("--vid=") => {
                args.vid = Some(parse_id(inline_value(other), "vendor")?);
            }
            other if other.starts_with("--pid=") => {
                args.pid = Some(parse_id(inline_value(other), "product")?);
            }
            // Convenience alias for the vendor this tool was built against.
            other if other.eq_ignore_ascii_case("asus") => args.vid = Some(ASUS_VID),
            other => return Err(format!("unexpected argument '{other}'\n{USAGE}")),
        }
    }

    Ok(args)
}

/// Pulls the value that follows a `--flag value` style argument.
fn take_value(flag: &str, rest: &mut impl Iterator<Item = String>) -> Result<String, String> {
    rest.next()
        .ok_or_else(|| format!("{flag} needs a hex id\n{USAGE}"))
}

/// Splits the value out of a `--flag=value` style argument.
fn inline_value(arg: &str) -> &str {
    let (_, value) = arg.split_once('=').expect("caller checked for the '=' prefix");
    value
}

/// Parses a vendor or product ID given in either base.
///
/// A `0x`/`0X` prefix selects hex; anything else is read as decimal — the
/// convention C, Rust literals, and most command-line tools use. Note that
/// HID identifiers are conventionally *written* in hex (`VID_0B05`,
/// `REV_0114`), so a bare `0B05` is rejected rather than quietly parsed as
/// something else; the `--detail` listing prints both bases (`0x0B05 (2821)`)
/// so either form can be copied straight back in.
///
/// `kind` only shapes the error message ("vendor" or "product").
fn parse_id(value: &str, kind: &str) -> Result<u16, String> {
    let parsed = match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        Some(digits) => u16::from_str_radix(digits, 16),
        None => value.parse::<u16>(),
    };
    parsed.map_err(|_| {
        format!("invalid {kind} id '{value}', expected decimal (2821) or hex (0x0B05)")
    })
}

/// Collapses the flat interface list into one entry per physical device.
///
/// A single physical device typically exposes several HID interfaces (one per
/// USB `MI_xx` sub-interface, further split per HID top-level `COLxx`
/// collection), so a raw interface listing badly overstates how much hardware
/// is attached. Grouping is by `(VID, PID)`, which means two identical units
/// of the same model would merge into one entry — an acceptable trade for how
/// much more readable the common case becomes.
///
/// Relies on `devices` already being sorted so that equal `(VID, PID)` pairs
/// are adjacent; it only ever compares against the group it is currently
/// building. Interfaces with no recoverable VID/PID at all share a single
/// `None` group.
fn group_by_product(devices: &[HidDevice]) -> Vec<(Option<(u16, u16)>, Vec<&HidDevice>)> {
    let mut groups: Vec<(Option<(u16, u16)>, Vec<&HidDevice>)> = Vec::new();
    for device in devices {
        let key = device.vendor_product_id();
        match groups.last_mut() {
            Some((group_key, interfaces)) if *group_key == key => interfaces.push(device),
            _ => groups.push((key, vec![device])),
        }
    }
    groups
}

/// Prints one device's summary: its name and ID, then its interface count.
fn print_group(key: Option<(u16, u16)>, interfaces: &[&HidDevice], pid_only: bool) {
    let id = match key {
        Some((_, pid)) if pid_only => format!("0x{pid:04X}"),
        Some((vid, pid)) => format!("0x{vid:04X}:0x{pid:04X}"),
        // No VID/PID recoverable at all — the device could not be opened and
        // its instance ID carries no VID_/PID_ tags (typical of non-USB
        // devices like an I2C-HID touchpad when running unelevated).
        None => "????".to_string(),
    };
    println!("{} ({id})", group_name(interfaces));

    let plural = if interfaces.len() == 1 {
        "interface"
    } else {
        "interfaces"
    };
    let unreadable = interfaces.iter().filter(|d| d.info.is_none()).count();
    if unreadable > 0 {
        println!("  {} {plural} ({unreadable} not readable)", interfaces.len());
    } else {
        println!("  {} {plural}", interfaces.len());
    }
}

/// Picks the most recognizable name for a group of interfaces.
///
/// Each interface carries its own name (`SPDRP_DEVICEDESC`, plus the HID
/// product string when readable), and within one physical device those differ:
/// the OEM's actual product name usually appears on only one or two of the
/// interfaces (e.g. "ROG STRIX SCOPE II"), while the rest get names Windows
/// generated from the collection's usage ("HID-compliant vendor-defined
/// device", "HID Keyboard Device").
///
/// So candidates are picked in three tiers, most to least informative:
///
/// 1. the most frequent name that does *not* look auto-generated — the OEM's
///    real product name, when any interface reports one;
/// 2. failing that, the most frequent auto-generated name that is not a
///    "vendor-defined" one, since those name a private data channel rather
///    than what the device actually is (a keyboard exposing more vendor
///    channels than keyboard collections would otherwise end up labelled
///    "HID-compliant vendor-defined device");
/// 3. failing that, simply the most frequent name of any kind.
fn group_name(interfaces: &[&HidDevice]) -> String {
    // (name, occurrences), in first-seen order so ties resolve deterministically.
    let mut candidates: Vec<(&str, usize)> = Vec::new();
    for device in interfaces {
        let names = [
            device.device_desc.as_deref(),
            device.info.as_ref().and_then(|i| i.product.as_deref()),
        ];
        for name in names.into_iter().flatten() {
            match candidates.iter_mut().find(|(known, _)| *known == name) {
                Some((_, count)) => *count += 1,
                None => candidates.push((name, 1)),
            }
        }
    }

    candidates
        .iter()
        .filter(|(name, _)| !is_generic_name(name))
        .max_by_key(|(_, count)| *count)
        .or_else(|| {
            candidates
                .iter()
                .filter(|(name, _)| !is_vendor_defined_name(name))
                .max_by_key(|(_, count)| *count)
        })
        .or_else(|| candidates.iter().max_by_key(|(_, count)| *count))
        .map(|(name, _)| name.to_string())
        .unwrap_or_else(|| "Unknown device".to_string())
}

/// Whether a name looks auto-generated by Windows rather than chosen by the
/// vendor. Every such name observed starts with "HID" — "HID-compliant mouse",
/// "HID Keyboard Device", "HIDI2C Device" — which real product names
/// ("ROG STRIX SCOPE II") do not.
fn is_generic_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('H' | 'h'), Some('I' | 'i'), Some('D' | 'd'))
    )
}

/// Whether an auto-generated name is one of the "vendor-defined" ones. These
/// name a private vendor data channel — the kind configuration software uses
/// for DPI, lighting, or macro settings — so they say nothing about what the
/// device is, and one device can expose several of them.
fn is_vendor_defined_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("vendor-defined")
}

/// Prints one numbered interface entry in a fixed multi-line layout
/// (`--detail` mode).
///
/// Two very different top halves depending on whether the interface could be
/// opened (see [`hid::HidDevice::open_error`]): full HID metadata when it
/// could, or just whatever VID/PID we could recover plus the reason it
/// failed when it couldn't. The bottom half (device description, instance
/// ID, path) is always SetupAPI-sourced and so always available regardless.
fn print_device(number: usize, device: &HidDevice) {
    let dash = "-".to_string();

    match &device.info {
        Some(info) => {
            println!(
                "[{number:>3}] {}:{} rev {:X}.{:02X}",
                hex_dec(info.vendor_id),
                hex_dec(info.product_id),
                // HidD_GetAttributes reports the version as one u16, which for
                // USB devices is the device descriptor's `bcdDevice` field.
                // That is binary-coded decimal: each nibble is one decimal
                // digit, so 0x0114 means version 1.14 — NOT 1.20. Hence the
                // hex formatting below, which reproduces the nibbles as
                // written and matches the `REV_0114` that Windows itself puts
                // in the device's hardware IDs. Vendors do occasionally store
                // non-BCD values here; printing the nibbles verbatim at least
                // keeps us consistent with what Windows reports.
                info.version >> 8,
                info.version & 0xFF,
            );
            println!(
                "      product      : {}",
                info.product.as_ref().unwrap_or(&dash)
            );
            println!(
                "      manufacturer : {}",
                info.manufacturer.as_ref().unwrap_or(&dash)
            );
            println!(
                "      serial       : {}",
                info.serial_number.as_ref().unwrap_or(&dash)
            );
            // Look up a friendly name for this collection's usage page/usage
            // (e.g. "Generic Desktop / Keyboard"); fall back to bare hex
            // when we don't recognize the pair.
            let described = usage::describe(info.usage_page, info.usage)
                .map(|name| format!("  ({name})"))
                .unwrap_or_default();
            println!(
                "      usage        : {}:{}{described}",
                hex_dec(info.usage_page),
                hex_dec(info.usage)
            );
            println!(
                "      report bytes : in {}, out {}, feature {}",
                info.input_report_len, info.output_report_len, info.feature_report_len
            );
        }
        None => {
            let reason = device
                .open_error
                .map(|code| format!("error {code}"))
                .unwrap_or_else(|| "unknown error".to_string());
            // Still show VID/PID when we can recover it from the instance ID
            // (see hid::HidDevice::vendor_product_id) — most useful exactly
            // for the access-denied primary keyboard/mouse collections,
            // which would otherwise show no identifying info at all.
            match device.vendor_product_id() {
                Some((vid, pid)) => println!(
                    "[{number:>3}] {}:{}  <not readable: {reason}>",
                    hex_dec(vid),
                    hex_dec(pid)
                ),
                None => println!("[{number:>3}] <not readable: {reason}>"),
            }
        }
    }

    println!(
        "      device desc  : {}",
        device.device_desc.as_ref().unwrap_or(&dash)
    );
    println!(
        "      instance id  : {}",
        device.instance_id.as_ref().unwrap_or(&dash)
    );
    println!("      path         : {}", device.path);
}

/// Formats a 16-bit identifier as hex plus decimal, e.g. `0x0B05 (2821)`.
///
/// HID/USB identifiers are conventionally written in hex (that is how vendor
/// databases, `REV_`/`VID_` hardware IDs, and usage tables spell them), but
/// the decimal value is what a WebHID `vendorId`, a `HIDP_CAPS` field read in
/// a debugger, or most scripting languages will show — so both are printed to
/// save cross-referencing them by hand.
fn hex_dec(value: u16) -> String {
    format!("0x{value:04X} ({value})")
}

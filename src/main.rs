//! Lists every HID device interface present on the system.
//!
//! Everything Windows-specific lives in [`win32`] (raw FFI) and [`hid`] (safe
//! wrappers) — this file is just the CLI: parse arguments, call
//! [`hid::enumerate`], optionally filter by vendor ID, sort for readable
//! grouping, and print.

mod hid;
mod usage;
mod win32;

use std::process::ExitCode;

use hid::HidDevice;

/// ASUSTek Computer Inc.'s USB vendor ID. Lets `asus` work as a shorthand for
/// `--vid 0B05` without having to remember/type the hex value.
const ASUS_VID: u16 = 0x0B05;

fn main() -> ExitCode {
    let vid_filter = match parse_args() {
        Ok(filter) => filter,
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

    if let Some(vid) = vid_filter {
        // `vendor_product_id()` also recovers VID/PID from the instance ID
        // for devices we couldn't open (see hid.rs), so devices Windows
        // locked (e.g. the OS's primary mouse/keyboard collections) still
        // get matched instead of silently vanishing from a vendor filter.
        devices.retain(|d| d.vendor_product_id().is_some_and(|(v, _)| v == vid));
    }

    // Group interfaces of the same physical device together: sort by
    // (VID, PID, usage page, usage) first so e.g. all of one mouse's
    // collections land next to each other in a stable, predictable order,
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
        println!("No matching HID device interfaces present.");
        return ExitCode::SUCCESS;
    }

    let unreadable = devices.iter().filter(|d| d.info.is_none()).count();
    // "present" for the full unfiltered listing, "matched" once a --vid/asus
    // filter is in play, so the count doesn't misleadingly imply it's the
    // system-wide total.
    let verb = if vid_filter.is_some() { "matched" } else { "present" };
    println!("{} HID device interface(s) {verb}.", devices.len());
    if unreadable > 0 {
        println!("{unreadable} could not be opened for querying.");
    }

    for (index, device) in devices.iter().enumerate() {
        println!();
        print_device(index + 1, device);
    }

    ExitCode::SUCCESS
}

/// Parses `--vid <hex>` / `--vendor <hex>` or the `asus` shortcut into a
/// vendor ID to filter on. `None` means "show everything".
///
/// Deliberately minimal hand-rolled parsing (no argument-parsing crate) to
/// match the rest of the project's "no dependencies" approach — there are
/// only three shapes of command line to recognize.
fn parse_args() -> Result<Option<u16>, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        // No arguments: no filter, list everything.
        [] => Ok(None),
        // Convenience alias for the vendor we've been debugging against.
        [flag] if flag.eq_ignore_ascii_case("asus") => Ok(Some(ASUS_VID)),
        // Accept an optional "0x"/"0X" prefix so both `0B05` and `0x0B05`
        // work as input.
        [flag, value] if flag == "--vid" || flag == "--vendor" => {
            u16::from_str_radix(value.trim_start_matches("0x").trim_start_matches("0X"), 16)
                .map(Some)
                .map_err(|_| format!("invalid vendor id '{value}', expected hex like 0B05"))
        }
        _ => Err("usage: hidctl [--vid <hex>] [asus]".to_string()),
    }
}

/// Prints one numbered device entry in a fixed multi-line layout.
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
                "[{number:>3}] {:04X}:{:04X}  rev {:X}.{:02X}",
                info.vendor_id,
                info.product_id,
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
                "      usage        : {:04X}:{:04X}{described}",
                info.usage_page, info.usage
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
                Some((vid, pid)) => {
                    println!("[{number:>3}] {vid:04X}:{pid:04X}  <not readable: {reason}>")
                }
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

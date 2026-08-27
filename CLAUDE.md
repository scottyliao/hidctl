# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`hidctl` is a Windows-only CLI that enumerates HID (Human Interface Device)
interfaces present on the system, groups them back into physical devices, and
prints a summary or a full per-interface dump. An `event` subcommand can also
open one device's vendor-defined report channel and stream its raw input
reports. It talks to `setupapi.dll`, `hid.dll`, and `kernel32.dll` directly
via hand-written FFI — there is no `windows`/`winapi` dependency, and
`Cargo.toml` has no dependencies at all. That "no external crates" choice is
deliberate and should be preserved; if a task seems to call for one, prefer
hand-rolling the extra bindings in `win32.rs` instead.

## Commands

```
cargo build              # debug build
cargo build --release    # release build
cargo run -- list                # all present HID devices, grouped summary
cargo run -- list --detail       # full per-interface dump
cargo run -- list asus           # shorthand for --vid 0x0B05
cargo run -- list --vid 0x0B05 --pid 0x1A92
cargo run -- event --vid 0x0B05 --pid 0x1AB3   # stream raw input reports (Ctrl+C stops)
cargo check               # fast type-check without codegen
cargo clippy              # lint (if clippy component is installed)
```

The CLI is subcommand-only: bare `hidctl` prints the command menu and exits
with failure rather than defaulting to `list`.

There is no test suite (`cargo test` has nothing to run) — this code is a
thin, mostly-`unsafe` wrapper around live Win32 device state, so it's
exercised by running it against real hardware rather than unit tests. When
verifying a change, prefer `cargo run -- list --detail` (and comparing
against Device Manager) over trying to add tests around the FFI layer.

Note some HID collections (notably the OS's primary keyboard/mouse) return
`ERROR_ACCESS_DENIED` unless the process is elevated — that's expected
behavior the tool already accounts for (see `HidDevice::open_error`), not a
bug to fix.

Builds only make sense on Windows (MSVC or GNU toolchain); `win32.rs` links
directly against `setupapi` and `hid` via `#[link(name = "...")]`.

## Git

Commit directly to `main`; do not create a working branch first (per the
repo owner's explicit preference).

## Architecture

Four modules, each with a single clear job — read them in this order when
tracing a change:

- **`win32.rs`** — raw, unsafe FFI declarations only. Every type, constant,
  and `extern "system"` signature is transcribed by hand from the Windows SDK
  headers (`setupapi.h`, `hidsdi.h`, `hidpi.h`, `hidclass.h`, `winnt.h`). No
  logic lives here beyond struct constructors that stamp the mandatory
  `cbSize`/`Size` self-description field. If a Win32 struct layout ever stops
  matching a future SDK, this is the file to check first — several structs
  have doc comments explaining exactly why their Rust layout has to diverge
  from `#[repr(C)]` defaults (e.g. `SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE`
  is hardcoded, not `size_of`-derived, because the real header packs that
  struct to non-default alignment).

- **`hid.rs`** — the safe layer. Converts the raw two-call-per-string Win32
  idioms (`SetupDiGetClassDevsW` → `SetupDiEnumDeviceInterfaces` →
  `SetupDiGetDeviceInterfaceDetailW`, the "ask twice" sizing pattern for
  variable-length strings, `CreateFileW` → `HidD_GetAttributes` /
  `HidD_GetPreparsedData` → `HidP_GetCaps`) into ordinary owned Rust values.
  RAII wrappers (`DevInfoSet`, `File`, `PreparsedData`) guarantee the matching
  `SetupDiDestroyDeviceInfoList` / `CloseHandle` / `HidD_FreePreparsedData`
  cleanup call runs exactly once. Its public surface is
  `enumerate() -> Result<Vec<HidDevice>>` plus the `HidDevice`/`HidInfo`
  types, and `EventStream` — a blocking `ReadFile` wrapper for the `event`
  subcommand, which opens with `GENERIC_READ` (unlike the zero-access open
  `enumerate` uses for metadata, chosen so system keyboards/mice held
  exclusively still answer `HidD_*` queries). A device that fails to open
  (commonly `ERROR_ACCESS_DENIED` on locked system keyboard/mouse
  collections) is still returned, with `info: None` and `open_error` set,
  rather than being dropped from the list.

- **`usage.rs`** — static lookup tables mapping HID usage page/usage numbers
  to human-readable names (e.g. `0x01:0x06` → "Generic Desktop / Keyboard").
  Pure data + lookup functions, no I/O. Only covers pages/usages likely to
  appear on a normal PC; anything unlisted still renders as plain hex.

- **`main.rs`** — the CLI: parses arguments (hand-rolled, no arg-parsing
  crate — matches the project's zero-dependency stance), calls
  `hid::enumerate`, filters by `--vid`/`--pid` (or the `asus` shorthand for
  `--vid 0x0B05`), sorts, then collapses the flat interface list into
  per-physical-device groups via `group_by_product`. The `event` subcommand
  is dispatched before the listing parser runs and has its own parser with
  different rules: `--vid`/`--pid` are *mandatory* there (a raw report
  carries no VID/PID, so the device must be chosen before opening), and the
  interface to read is the one also matching the hardcoded vendor-defined
  usage `TARGET_USAGE_PAGE:TARGET_USAGE` (`0xFFC0:0x0001`). The read loop
  relies on the OS default Ctrl+C handling — no console handler is installed.

### Key data-flow detail: interfaces vs. devices

A single physical HID device (e.g. a gaming mouse) commonly exposes *several*
device interfaces — one per USB `MI_xx` sub-interface, further split per HID
top-level `COLxx` collection. `hid::enumerate()` returns the flat,
one-per-interface list; `main.rs` sorts it by `(VID, PID, usage page, usage)`
so same-device interfaces land contiguously, then `group_by_product` collapses
runs of matching `(VID, PID)` back into one printed entry. Grouping by
`(VID, PID)` alone means two identical units of the same model merge into a
single entry — an accepted trade-off for readability.

`HidDevice::vendor_product_id()` also recovers VID/PID by parsing the
`HID\VID_xxxx&PID_yyyy&...` instance ID for devices that couldn't be opened
(and thus have no `HidInfo`). This matters because the devices most likely to
fail to open — the OS's primary keyboard/mouse collections — are exactly the
ones a `--vid`/`asus` filter would otherwise silently drop.

`group_name()` picks the most recognizable name for a device out of its
interfaces' names, in three tiers: a non-generic OEM product name first, then
a generic-but-not-"vendor-defined" Windows-assigned name, then whatever's left
— since one physical device's real product name usually appears on only one
or two of its several interfaces.

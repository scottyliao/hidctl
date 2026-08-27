//! Raw declarations for the Win32 APIs we need.
//!
//! Hand-written bindings only: `kernel32` for handles, `setupapi` for device
//! interface enumeration, and `hid` for the HID class driver helpers.
//!
//! This module intentionally does not depend on any external crate (no
//! `windows`/`winapi`). Every type, constant, and `extern "system"` function
//! signature below is transcribed by hand from the Windows SDK headers
//! (`setupapi.h`, `hidsdi.h`, `hidpi.h`, `hidclass.h`, `winnt.h`) — so this
//! file is the single place to check against the SDK if something ever
//! stops matching a future Windows version.
//!
//! Everything here is a thin, unsafe mirror of the C API: no lifetime
//! tracking, no RAII, no error translation. [`crate::hid`] is the layer that
//! turns these raw calls into something safe to use from [`crate::main`].

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::c_void;
use core::mem::size_of;

// ---------------------------------------------------------------------------
// Primitive type aliases
//
// Win32 spells out its own integer/pointer typedefs; we mirror the exact
// underlying representation so `#[repr(C)]` structs and `extern "system"`
// signatures line up byte-for-byte with what the real DLLs expect.
// ---------------------------------------------------------------------------

/// Win32 `BOOL`: a 4-byte int, non-zero means `TRUE`. Note this is a
/// different width from Rust's `bool` (1 byte) — never transmute between them.
pub type BOOL = i32;
/// Win32 `BOOLEAN` (used by the `HidD_*`/`HidP_*` family): a 1-byte flag,
/// non-zero means `TRUE`. Distinct from `BOOL` above — different APIs use
/// different truthy widths, so both are declared to match each call site.
pub type BOOLEAN = u8;
/// `NTSTATUS`: the signed 32-bit status code used by `HidP_*` calls. Success
/// is `0` or a small positive "warning" value; see [`HIDP_STATUS_SUCCESS`].
pub type NTSTATUS = i32;
/// A generic kernel object handle (file handles, etc.). Always opaque to us —
/// we only ever pass it back to the API that produced it, or to `CloseHandle`.
pub type HANDLE = *mut c_void;
/// Handle to a SetupAPI "device information set" — the in-memory collection
/// of devices/interfaces produced by `SetupDiGetClassDevsW`.
pub type HDEVINFO = *mut c_void;
/// Opaque pointer to the parsed HID report descriptor blob produced by
/// `HidD_GetPreparsedData`. We never look inside it ourselves; it is only
/// ever handed to other `HidP_*` calls (here, just `HidP_GetCaps`).
pub type PHIDP_PREPARSED_DATA = *mut c_void;
/// A HID "usage" value (16-bit numeric ID within a usage page), e.g. `0x02`
/// for Mouse within the Generic Desktop page. See [`crate::usage`].
pub type USAGE = u16;

/// The sentinel `CreateFileW`/`SetupDiGetClassDevsW` return instead of a real
/// handle on failure. It is `(HANDLE)-1`, i.e. all bits set — NOT `NULL` — so
/// it must be compared explicitly; a null-pointer check alone is not enough.
pub const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;

// ---------------------------------------------------------------------------
// GetLastError() codes we branch on
// ---------------------------------------------------------------------------

/// `ERROR_INSUFFICIENT_BUFFER` (122): "your buffer is too small". We rely on
/// this specific code as the *expected* outcome of a size-probing call (pass
/// a zero-length buffer, read back the required size), so it is treated as
/// success-with-a-number rather than a real failure. See
/// [`crate::hid::interface_detail`] and [`crate::hid::device_desc`].
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
/// `ERROR_NO_MORE_ITEMS` (259): the enumeration loop's natural termination —
/// `SetupDiEnumDeviceInterfaces` returns this once `MemberIndex` runs past
/// the last device interface in the set.
pub const ERROR_NO_MORE_ITEMS: u32 = 259;

// ---------------------------------------------------------------------------
// CreateFileW flags
// ---------------------------------------------------------------------------

/// Allow other handles to open the same file/device for reading concurrently.
pub const FILE_SHARE_READ: u32 = 0x0000_0001;
/// Allow other handles to open the same file/device for writing concurrently.
/// We pass both share flags when opening a HID device so we never contend
/// with whatever else (input stack, other tools) already has it open.
pub const FILE_SHARE_WRITE: u32 = 0x0000_0002;
/// `dwCreationDisposition`: open the device only if it already exists (as
/// opposed to creating a new file) — the only sensible choice for a device
/// object that the OS itself creates and destroys as hardware comes and goes.
pub const OPEN_EXISTING: u32 = 3;

// ---------------------------------------------------------------------------
// SetupDiGetClassDevsW flags
// ---------------------------------------------------------------------------

/// `DIGCF_PRESENT`: only return devices that are currently attached and
/// enabled. Without this flag SetupAPI would also return "ghost" devices —
/// ones Windows still remembers in the registry but that are not plugged in
/// or are disabled in Device Manager.
pub const DIGCF_PRESENT: u32 = 0x0000_0002;
/// `DIGCF_DEVICEINTERFACE`: enumerate *device interfaces* of the given
/// interface class GUID (what we pass is `GUID_DEVINTERFACE_HID`), rather
/// than enumerating device *nodes* of a setup class. This is what makes
/// `SetupDiEnumDeviceInterfaces` (as opposed to `SetupDiEnumDeviceInfo`) the
/// right enumerator to pair it with.
pub const DIGCF_DEVICEINTERFACE: u32 = 0x0000_0010;

// ---------------------------------------------------------------------------
// SetupDiGetDeviceRegistryPropertyW property IDs
// ---------------------------------------------------------------------------

/// `SPDRP_DEVICEDESC`: the device's friendly description string — the same
/// text Device Manager shows as the node's display name (e.g. "HID-compliant
/// mouse").
pub const SPDRP_DEVICEDESC: u32 = 0x0000_0000;

// ---------------------------------------------------------------------------
// HidP_* status codes
// ---------------------------------------------------------------------------

/// `HIDP_STATUS_SUCCESS`: the only `NTSTATUS` value from `HidP_GetCaps` that
/// means the `HIDP_CAPS` output is valid. Encoded as an NTSTATUS "success"
/// facility code, not `0`, so it can't be checked with a simple zero test.
pub const HIDP_STATUS_SUCCESS: NTSTATUS = 0x0011_0000;

// ---------------------------------------------------------------------------
// Structs
//
// Every struct below is `#[repr(C)]` so the Rust field layout matches the C
// ABI exactly — field order, padding, and alignment all have to agree with
// what the DLL was compiled against, since we're handing it raw pointers
// into these structs.
// ---------------------------------------------------------------------------

/// The classic 16-byte Windows GUID (`{Data1-Data2-Data3-Data4}`), used both
/// as the HID device interface class GUID and inside a couple of SetupAPI
/// structs that carry a GUID field.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GUID {
    pub Data1: u32,
    pub Data2: u16,
    pub Data3: u16,
    pub Data4: [u8; 8],
}

/// One entry from `SetupDiEnumDeviceInterfaces`: identifies a single device
/// interface (not yet its path — that requires a follow-up call to
/// `SetupDiGetDeviceInterfaceDetailW`, see [`crate::hid::interface_detail`]).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SP_DEVICE_INTERFACE_DATA {
    /// Must be set to `size_of::<Self>()` before calling into any SetupAPI
    /// function that takes this struct — this is how these APIs version
    /// their structs across SDK releases. See [`Self::new`].
    pub cbSize: u32,
    pub InterfaceClassGuid: GUID,
    pub Flags: u32,
    /// Driver-reserved; must not be touched by callers.
    pub Reserved: usize,
}

impl SP_DEVICE_INTERFACE_DATA {
    /// Zero-initialized except for the mandatory `cbSize` stamp.
    pub fn new() -> Self {
        Self {
            cbSize: size_of::<Self>() as u32,
            InterfaceClassGuid: GUID::default(),
            Flags: 0,
            Reserved: 0,
        }
    }
}

/// Identifies one device *node* (as opposed to one of its interfaces) inside
/// a device information set. Filled in as an out-parameter by
/// `SetupDiGetDeviceInterfaceDetailW`, then reused to query registry
/// properties (`SetupDiGetDeviceRegistryPropertyW`) and the instance ID
/// (`SetupDiGetDeviceInstanceIdW`) for that same underlying device.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SP_DEVINFO_DATA {
    /// Same `size_of::<Self>()` self-description convention as
    /// [`SP_DEVICE_INTERFACE_DATA::cbSize`].
    pub cbSize: u32,
    pub ClassGuid: GUID,
    /// Opaque device instance handle (`DEVINST`) — not something we ever
    /// decode ourselves; only ever passed back into SetupAPI calls.
    pub DevInst: u32,
    /// Driver-reserved; must not be touched by callers.
    pub Reserved: usize,
}

impl SP_DEVINFO_DATA {
    /// Zero-initialized except for the mandatory `cbSize` stamp.
    pub fn new() -> Self {
        Self {
            cbSize: size_of::<Self>() as u32,
            ClassGuid: GUID::default(),
            DevInst: 0,
            Reserved: 0,
        }
    }
}

/// The device interface's symbolic path (e.g. `\\?\hid#vid_...&pid_...#...`),
/// as filled in by `SetupDiGetDeviceInterfaceDetailW`.
///
/// This mirrors `SP_DEVICE_INTERFACE_DETAIL_DATA_W` from `setupapi.h`, which
/// is a genuinely variable-length struct in C: `DevicePath` is declared as a
/// 1-element array but the API always allocates (and writes) however many
/// `u16`s the actual path needs, NUL included, immediately following
/// `cbSize` in memory. Because of that:
///
/// - We never construct or read this type by value — only by casting a raw,
///   sufficiently large buffer to `*mut Self` and reading through the
///   pointer. See [`crate::hid::interface_detail`].
/// - `size_of::<Self>()` is meaningless for sizing the buffer; the buffer
///   size instead comes from the required-size probe the API itself reports.
#[repr(C)]
pub struct SP_DEVICE_INTERFACE_DETAIL_DATA_W {
    pub cbSize: u32,
    pub DevicePath: [u16; 1],
}

/// The `cbSize` value `SetupDiGetDeviceInterfaceDetailW` actually expects for
/// [`SP_DEVICE_INTERFACE_DETAIL_DATA_W`].
///
/// This is deliberately *not* `size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>()`.
/// The real `setupapi.h` header packs this particular struct to 1-byte
/// alignment, so its "as the compiler that built setupapi.dll saw it" size is
/// 6 bytes on 32-bit (a `u32` plus one `u16`, no padding) and 8 bytes on
/// 64-bit (the same fields, but padded so the trailing array stays
/// `u16`-aligned) — whereas Rust's default `#[repr(C)]` layout for the struct
/// above would pad `DevicePath` up to 4-byte alignment on 32-bit and report a
/// different `size_of`. Passing the wrong constant here makes the call fail
/// validation inside SetupAPI, so the value is hardcoded to match the SDK
/// header rather than derived from Rust's own layout of the struct.
pub const SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE: u32 =
    if size_of::<usize>() == 8 { 8 } else { 6 };

/// VID/PID/version for one opened HID device, as returned by
/// `HidD_GetAttributes`. This is the cheapest way to get VID/PID — it needs
/// no report-descriptor parsing, just an open handle.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HIDD_ATTRIBUTES {
    /// Must be `size_of::<Self>()`, same self-describing convention as the
    /// SetupAPI structs above.
    pub Size: u32,
    pub VendorID: u16,
    pub ProductID: u16,
    pub VersionNumber: u16,
}

impl HIDD_ATTRIBUTES {
    /// Zero-initialized except for the mandatory `Size` stamp.
    pub fn new() -> Self {
        Self {
            Size: size_of::<Self>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        }
    }
}

/// Capability summary for a HID top-level collection, produced by
/// `HidP_GetCaps` from the device's parsed report descriptor
/// ([`PHIDP_PREPARSED_DATA`]).
///
/// We only read the first few fields (`Usage`, `UsagePage`, and the three
/// report byte lengths); the rest exist purely so this struct's memory layout
/// matches the real `HIDP_CAPS` from `hidpi.h` byte-for-byte — `HidP_GetCaps`
/// writes the whole struct, so every field between the ones we use and the
/// end has to be declared (even though [`crate::hid`] never reads them) or
/// later fields would land at the wrong offsets.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct HIDP_CAPS {
    pub Usage: USAGE,
    pub UsagePage: USAGE,
    pub InputReportByteLength: u16,
    pub OutputReportByteLength: u16,
    pub FeatureReportByteLength: u16,
    /// Padding matching `hidpi.h`'s `Reserved[17]` — unused, but required for
    /// layout parity with the fields that follow it.
    pub Reserved: [u16; 17],
    pub NumberLinkCollectionNodes: u16,
    pub NumberInputButtonCaps: u16,
    pub NumberInputValueCaps: u16,
    pub NumberInputDataIndices: u16,
    pub NumberOutputButtonCaps: u16,
    pub NumberOutputValueCaps: u16,
    pub NumberOutputDataIndices: u16,
    pub NumberFeatureButtonCaps: u16,
    pub NumberFeatureValueCaps: u16,
    pub NumberFeatureDataIndices: u16,
}

// ---------------------------------------------------------------------------
// kernel32.dll
// ---------------------------------------------------------------------------

unsafe extern "system" {
    /// Returns the calling thread's last-error code, set by whichever Win32
    /// call most recently failed. Must be called immediately after a failing
    /// call — any intervening Win32 call (including ones Rust's standard
    /// library might make) can overwrite it.
    pub fn GetLastError() -> u32;

    /// Closes any kernel handle — here, always one previously returned by
    /// [`CreateFileW`]. Safe to call exactly once per handle; see
    /// [`crate::hid::File`]'s `Drop` impl, which is the only caller.
    pub fn CloseHandle(hObject: HANDLE) -> BOOL;

    /// General-purpose "open a handle to a file or device object" call. We
    /// only ever use it against HID device interface paths (never real
    /// files), with `dwDesiredAccess = 0` — see
    /// [`crate::hid::File::open_for_query`] for why zero access is
    /// deliberate.
    pub fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: HANDLE,
    ) -> HANDLE;
}

// ---------------------------------------------------------------------------
// setupapi.dll
//
// The PnP configuration manager's user-mode API surface: builds and walks
// "device information sets" — snapshots of devices and/or device interfaces
// matching some filter (here, "present interfaces of the HID class GUID").
// ---------------------------------------------------------------------------

#[link(name = "setupapi")]
unsafe extern "system" {
    /// Builds a device information set containing every device interface
    /// (because we pass `DIGCF_DEVICEINTERFACE`) of the class identified by
    /// `ClassGuid` that is currently present (`DIGCF_PRESENT`). This is the
    /// entry point for the whole enumeration in
    /// [`crate::hid::DevInfoSet::present_interfaces`]; the set it returns
    /// must later be released with [`SetupDiDestroyDeviceInfoList`].
    ///
    /// `Enumerator` and `hwndParent` are both left null: we don't restrict to
    /// a particular bus enumerator (we want USB, Bluetooth, I2C-HID, ACPI —
    /// everything), and we have no parent window to report UI against.
    pub fn SetupDiGetClassDevsW(
        ClassGuid: *const GUID,
        Enumerator: *const u16,
        hwndParent: *mut c_void,
        Flags: u32,
    ) -> HDEVINFO;

    /// Frees a device information set previously returned by
    /// [`SetupDiGetClassDevsW`]. Called exactly once, from
    /// [`crate::hid::DevInfoSet`]'s `Drop` impl.
    pub fn SetupDiDestroyDeviceInfoList(DeviceInfoSet: HDEVINFO) -> BOOL;

    /// Fetches the `MemberIndex`-th device interface out of `DeviceInfoSet`
    /// that matches `InterfaceClassGuid`. Called in a loop with
    /// `MemberIndex` counting up from `0` until it fails with
    /// [`ERROR_NO_MORE_ITEMS`] — the standard SetupAPI enumeration idiom, see
    /// [`crate::hid::enumerate`].
    pub fn SetupDiEnumDeviceInterfaces(
        DeviceInfoSet: HDEVINFO,
        DeviceInfoData: *mut SP_DEVINFO_DATA,
        InterfaceClassGuid: *const GUID,
        MemberIndex: u32,
        DeviceInterfaceData: *mut SP_DEVICE_INTERFACE_DATA,
    ) -> BOOL;

    /// Two jobs in one function, selected by whether `DeviceInterfaceDetailData`
    /// is null:
    /// - Called with a null/zero-sized buffer, it fails with
    ///   [`ERROR_INSUFFICIENT_BUFFER`] and reports the buffer size actually
    ///   needed through `RequiredSize` — the standard "ask twice" pattern for
    ///   any Win32 API returning a variable-length string.
    /// - Called again with a large-enough buffer, it fills in the device
    ///   interface's symbolic path (into the variable-length
    ///   [`SP_DEVICE_INTERFACE_DETAIL_DATA_W`]) and, as a bonus, the
    ///   [`SP_DEVINFO_DATA`] for the underlying device node — so one call
    ///   gets us both the path and the handle we need for later registry /
    ///   instance-ID queries.
    ///
    /// See [`crate::hid::interface_detail`] for both calls back to back.
    pub fn SetupDiGetDeviceInterfaceDetailW(
        DeviceInfoSet: HDEVINFO,
        DeviceInterfaceData: *mut SP_DEVICE_INTERFACE_DATA,
        DeviceInterfaceDetailData: *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W,
        DeviceInterfaceDetailDataSize: u32,
        RequiredSize: *mut u32,
        DeviceInfoData: *mut SP_DEVINFO_DATA,
    ) -> BOOL;

    /// Reads the device's instance ID string (e.g.
    /// `HID\VID_0B05&PID_1A92&MI_01\8&1BB0B39&0&0000`) — the same identifier
    /// Device Manager shows under a device's Details tab. Also follows the
    /// ask-twice sizing pattern; see [`crate::hid::instance_id`].
    pub fn SetupDiGetDeviceInstanceIdW(
        DeviceInfoSet: HDEVINFO,
        DeviceInfoData: *mut SP_DEVINFO_DATA,
        DeviceInstanceId: *mut u16,
        DeviceInstanceIdSize: u32,
        RequiredSize: *mut u32,
    ) -> BOOL;

    /// Reads one legacy `SPDRP_*` registry property for a device node — here
    /// used only for [`SPDRP_DEVICEDESC`]. Also follows the ask-twice sizing
    /// pattern; see [`crate::hid::device_desc`]. `PropertyRegDataType` is
    /// left null in both calls because we already know the property is a
    /// string and don't need the registry type tag echoed back.
    pub fn SetupDiGetDeviceRegistryPropertyW(
        DeviceInfoSet: HDEVINFO,
        DeviceInfoData: *mut SP_DEVINFO_DATA,
        Property: u32,
        PropertyRegDataType: *mut u32,
        PropertyBuffer: *mut u8,
        PropertyBufferSize: u32,
        RequiredSize: *mut u32,
    ) -> BOOL;
}

// ---------------------------------------------------------------------------
// hid.dll
//
// The HID class driver's user-mode helper library: identifies the HID device
// interface class GUID, reads device metadata (VID/PID, strings) from an
// open handle, and parses report descriptors.
// ---------------------------------------------------------------------------

#[link(name = "hid")]
unsafe extern "system" {
    /// Writes `GUID_DEVINTERFACE_HID` — the well-known device interface class
    /// GUID (`{4D1E55B2-F16F-11CF-88CB-001111000030}`) shared by *every*
    /// device that registers under the standard HID class driver stack,
    /// regardless of transport (USB, Bluetooth, I2C-HID, or an ACPI virtual
    /// HID device). This is what we pass to [`SetupDiGetClassDevsW`] to make
    /// the enumeration HID-wide instead of scoped to one bus.
    pub fn HidD_GetHidGuid(HidGuid: *mut GUID);

    /// Reads VID/PID/version from an *opened* HID device handle. Requires no
    /// access rights beyond having successfully opened the handle at all
    /// (even with `dwDesiredAccess = 0`).
    pub fn HidD_GetAttributes(HidDeviceObject: HANDLE, Attributes: *mut HIDD_ATTRIBUTES)
    -> BOOLEAN;

    /// Reads the device's manufacturer string (from its USB/HID string
    /// descriptors, when it has one) as UTF-16 into `Buffer`. Fails (returns
    /// `0`/`FALSE`) if the device doesn't expose this string at all — not
    /// every HID device does. See [`crate::hid::wide_string_query`], which
    /// wraps this and the two functions below identically.
    pub fn HidD_GetManufacturerString(
        HidDeviceObject: HANDLE,
        Buffer: *mut c_void,
        BufferLength: u32,
    ) -> BOOLEAN;

    /// Same shape as [`HidD_GetManufacturerString`], for the product name
    /// string.
    pub fn HidD_GetProductString(
        HidDeviceObject: HANDLE,
        Buffer: *mut c_void,
        BufferLength: u32,
    ) -> BOOLEAN;

    /// Same shape as [`HidD_GetManufacturerString`], for the serial number
    /// string.
    pub fn HidD_GetSerialNumberString(
        HidDeviceObject: HANDLE,
        Buffer: *mut c_void,
        BufferLength: u32,
    ) -> BOOLEAN;

    /// Retrieves the device's parsed report descriptor as an opaque blob
    /// ([`PHIDP_PREPARSED_DATA`]). The only thing we do with it is hand it
    /// straight to [`HidP_GetCaps`]; it must be released with
    /// [`HidD_FreePreparsedData`] when done — see
    /// [`crate::hid::PreparsedData`].
    pub fn HidD_GetPreparsedData(
        HidDeviceObject: HANDLE,
        PreparsedData: *mut PHIDP_PREPARSED_DATA,
    ) -> BOOLEAN;

    /// Releases a preparsed-data blob obtained from [`HidD_GetPreparsedData`].
    pub fn HidD_FreePreparsedData(PreparsedData: PHIDP_PREPARSED_DATA) -> BOOLEAN;

    /// Extracts the top-level collection's capability summary (usage page,
    /// usage, and input/output/feature report byte lengths, among other
    /// fields we don't read) out of a preparsed report descriptor. Returns
    /// [`HIDP_STATUS_SUCCESS`] on success — any other `NTSTATUS` means
    /// `Capabilities` was not filled in.
    pub fn HidP_GetCaps(
        PreparsedData: PHIDP_PREPARSED_DATA,
        Capabilities: *mut HIDP_CAPS,
    ) -> NTSTATUS;
}

//! Safe wrappers around the SetupAPI / HID class driver calls in [`crate::win32`].
//!
//! The public surface of this module is [`enumerate`] plus the data types it
//! produces ([`HidDevice`], [`HidInfo`]), and [`EventStream`] for the `event`
//! subcommand's raw-report reading. Everything else here exists to turn the
//! raw, unsafe, two-call-per-string Win32 idioms in [`crate::win32`] into
//! ordinary owned Rust values (`String`, `Vec`, `Option`) with RAII cleanup,
//! so [`crate::main`] never has to see a raw handle, pointer, or `unsafe`
//! block.

use core::mem::{offset_of, size_of};
use core::ptr;
use std::fmt;

use crate::win32::*;

/// A Win32 call that failed, with the error code it reported.
///
/// We don't attempt to translate `code` into a message ourselves (that would
/// mean re-implementing `FormatMessageW`); it is printed as a bare number and
/// left to the reader to look up (as we did in conversation for `error 5` =
/// `ERROR_ACCESS_DENIED`).
#[derive(Debug)]
pub struct Error {
    /// Name of the Win32/HID API that failed, for context in the message.
    pub call: &'static str,
    /// The value `GetLastError()` (or, for `HidP_GetCaps`, the raw
    /// `NTSTATUS`) reported right after the failing call.
    pub code: u32,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed (error {})", self.call, self.code)
    }
}

impl std::error::Error for Error {}

/// Shorthand for `Result<T, Error>`, used throughout this module.
pub type Result<T> = std::result::Result<T, Error>;

/// Captures `GetLastError()` right after a failing call and tags it with the
/// name of that call. Must be invoked immediately after the failure — any
/// other Win32 call in between (even one made by the Rust runtime) can
/// clobber the thread-local error code first.
fn last_error(call: &'static str) -> Error {
    Error {
        call,
        code: unsafe { GetLastError() },
    }
}

/// Attributes readable only after the device interface is opened.
///
/// Split out from [`HidDevice`] because everything in here comes from a
/// `CreateFileW` handle succeeding — a device we couldn't open (`error 5` /
/// `ACCESS_DENIED` being the common case for the OS's primary keyboard/mouse
/// collections) simply has no `HidInfo`, but is still listed with whatever
/// SetupAPI could tell us without opening it.
#[derive(Debug, Clone)]
pub struct HidInfo {
    /// USB (or USB-style) vendor ID, e.g. `0x0B05` for ASUSTek.
    pub vendor_id: u16,
    pub product_id: u16,
    /// Device-reported firmware/hardware version as a raw BCD-ish `u16`;
    /// [`crate::main::print_device`] splits it into `major.minor` by
    /// shifting/masking the high and low byte.
    pub version: u16,
    /// `None` when the device has no manufacturer string descriptor at all —
    /// this is normal for plenty of HID devices, not a failure.
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    /// HID usage page for this top-level collection (e.g. `0x01` = Generic
    /// Desktop). See [`crate::usage::page_name`].
    pub usage_page: u16,
    /// HID usage within that page (e.g. `0x06` = Keyboard, within Generic
    /// Desktop). See [`crate::usage::usage_name`].
    pub usage: u16,
    pub input_report_len: u16,
    pub output_report_len: u16,
    pub feature_report_len: u16,
}

/// One HID device interface as reported by the device interface enumerator.
///
/// "Device interface" is the operative word: a single physical gadget
/// commonly shows up as *several* of these (one per USB "MI_xx" sub-interface,
/// further split into one per HID top-level "COLxx" collection within it) —
/// see the multi-interface breakdown of e.g. a composite gaming mouse we
/// walked through in conversation. Group by [`Self::vendor_product_id`] to
/// reconstruct "physical devices" from a flat list of these.
#[derive(Debug, Clone)]
pub struct HidDevice {
    /// The `\\?\hid#...` symbolic link path — the string you'd pass to
    /// `CreateFileW` yourself to open this exact interface.
    pub path: String,
    /// PnP instance ID, e.g. `HID\VID_0B05&PID_1A92&MI_01\8&1BB0B39&0&0000` —
    /// the same string Device Manager shows under a device's Details tab.
    /// `None` only if SetupAPI itself refused to report it (rare).
    pub instance_id: Option<String>,
    /// The registry `SPDRP_DEVICEDESC` string — Device Manager's display
    /// name for the underlying device node (e.g. "ROG STRIX SCOPE II").
    pub device_desc: Option<String>,
    /// `None` when the interface could not be opened; `open_error` says why.
    pub info: Option<HidInfo>,
    /// The `GetLastError()` code from the failed `CreateFileW`/`HidD_GetAttributes`
    /// call, when `info` is `None`. The value we kept running into for the
    /// OS's primary keyboard/mouse collections is `5` (`ERROR_ACCESS_DENIED`):
    /// Windows locks those specific collections to non-elevated processes so
    /// ordinary user-mode code can't raw-read keystrokes/mouse movement as a
    /// keylogger would.
    pub open_error: Option<u32>,
}

impl HidDevice {
    /// VID/PID either from the opened attributes, or parsed back out of the
    /// instance ID (`HID\VID_xxxx&PID_yyyy&...`) for devices we couldn't
    /// open.
    ///
    /// This fallback matters because the devices most likely to fail to open
    /// (the primary keyboard/mouse collections, see [`Self::open_error`])
    /// are exactly the ones you'd most want identified — without it, an
    /// `asus`/`--vid` filter in [`crate::main`] would silently drop every
    /// ASUS mouse/keyboard collection that Windows keeps locked.
    pub fn vendor_product_id(&self) -> Option<(u16, u16)> {
        if let Some(info) = &self.info {
            return Some((info.vendor_id, info.product_id));
        }
        parse_vid_pid(self.instance_id.as_deref()?)
    }
}

/// Extracts `(VID, PID)` from a `HID\VID_xxxx&PID_yyyy&...` instance ID.
///
/// Instance IDs are plain ASCII and always spell the vendor/product ID as
/// exactly 4 uppercase hex digits right after the `VID_`/`PID_` tag, so a
/// straightforward substring search plus a fixed-width slice is enough —
/// no need for a general parser.
fn parse_vid_pid(instance_id: &str) -> Option<(u16, u16)> {
    let vid_pos = instance_id.find("VID_")?;
    let vid = u16::from_str_radix(instance_id.get(vid_pos + 4..vid_pos + 8)?, 16).ok()?;
    let pid_pos = instance_id.find("PID_")?;
    let pid = u16::from_str_radix(instance_id.get(pid_pos + 4..pid_pos + 8)?, 16).ok()?;
    Some((vid, pid))
}

/// Owns the device information set returned by `SetupDiGetClassDevsW`.
///
/// A thin RAII wrapper: its only job is to guarantee
/// `SetupDiDestroyDeviceInfoList` runs exactly once, even if an error return
/// somewhere in [`enumerate`] causes an early exit via `?`.
struct DevInfoSet(HDEVINFO);

impl DevInfoSet {
    /// Builds the set of every currently-present device interface belonging
    /// to `class` (in practice, always `GUID_DEVINTERFACE_HID` from
    /// `HidD_GetHidGuid`). See [`DIGCF_PRESENT`]/[`DIGCF_DEVICEINTERFACE`]
    /// in [`crate::win32`] for exactly what these flags restrict us to.
    fn present_interfaces(class: &GUID) -> Result<Self> {
        let handle = unsafe {
            SetupDiGetClassDevsW(
                class,
                ptr::null(),
                ptr::null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        // SetupDiGetClassDevsW signals failure with INVALID_HANDLE_VALUE
        // (all bits set), not NULL — both are checked defensively.
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(last_error("SetupDiGetClassDevsW"));
        }
        Ok(Self(handle))
    }
}

impl Drop for DevInfoSet {
    fn drop(&mut self) {
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// Owns a handle opened with `CreateFileW`.
struct File(HANDLE);

impl File {
    /// Shared `CreateFileW` call behind [`Self::open_for_query`] and
    /// [`Self::open_for_read`] — the two only ever differ in
    /// `dwDesiredAccess`.
    fn open(path: &[u16], access: u32) -> Result<Self> {
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateFileW"));
        }
        Ok(Self(handle))
    }

    /// Opens a device interface for querying only.
    ///
    /// `dwDesiredAccess` is deliberately 0: Windows keeps system keyboards and
    /// mice open exclusively, so asking for read/write access would fail on
    /// exactly the devices we most want to describe. Zero access is still
    /// enough for the `HidD_*` metadata queries.
    ///
    /// Even so, three specific top-level collections on this machine still
    /// refused *any* access (`ERROR_ACCESS_DENIED`) until the whole process
    /// was run elevated — the primary Mouse/Keyboard collections carry a
    /// security descriptor that blocks non-administrator opens outright,
    /// independent of the access mask requested. See [`HidDevice::open_error`].
    fn open_for_query(path: &[u16]) -> Result<Self> {
        Self::open(path, 0)
    }

    /// Opens a device interface for reading raw input reports via
    /// [`ReadFile`] (see [`EventStream`]) — needs [`GENERIC_READ`], unlike
    /// the zero-access open above which only ever feeds `HidD_*` metadata
    /// calls.
    fn open_for_read(path: &[u16]) -> Result<Self> {
        Self::open(path, GENERIC_READ)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A HID device interface opened for reading raw input reports, as used by
/// the `event` subcommand (`hidctl event --vid <id> --pid <id>`).
///
/// Reading needs an actual `GENERIC_READ` handle rather than the zero-access
/// one [`enumerate`] uses for metadata — see [`File::open_for_read`].
pub struct EventStream {
    file: File,
    /// Byte length of one `ReadFile` call's buffer. Taken from the matched
    /// interface's `HidInfo::input_report_len` (`HIDP_CAPS::InputReportByteLength`),
    /// which already accounts for the leading report-ID byte on devices that
    /// number their reports — the buffer size `ReadFile` itself expects.
    report_len: usize,
}

impl EventStream {
    /// Opens `path` (a [`HidDevice::path`]) for reading, sizing every read
    /// from `report_len` (a matched device's `HidInfo::input_report_len`).
    pub fn open(path: &str, report_len: u16) -> Result<Self> {
        // `CreateFileW` wants a NUL-terminated wide string, same as the
        // metadata-query path in `describe`/`interface_detail`.
        let mut wide: Vec<u16> = path.encode_utf16().collect();
        wide.push(0);
        Ok(Self {
            file: File::open_for_read(&wide)?,
            report_len: report_len as usize,
        })
    }

    /// Blocks until the device emits one input report, then returns its raw
    /// bytes (report ID first, if the device numbers its reports).
    ///
    /// No timeout, no polling: the handle was opened without
    /// `FILE_FLAG_OVERLAPPED`, so `ReadFile` itself blocks the calling
    /// thread until data arrives — which is exactly what the `event`
    /// subcommand's infinite loop wants.
    pub fn read_report(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.report_len];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                self.file.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("ReadFile"));
        }
        buf.truncate(read as usize);
        Ok(buf)
    }
}

/// Owns the report descriptor blob returned by `HidD_GetPreparsedData`.
struct PreparsedData(PHIDP_PREPARSED_DATA);

impl PreparsedData {
    /// Fetches the preparsed report descriptor for an already-open device
    /// handle. Must be paired with `HidD_FreePreparsedData` — handled by
    /// this type's `Drop` impl below.
    fn get(device: &File) -> Result<Self> {
        let mut data: PHIDP_PREPARSED_DATA = ptr::null_mut();
        if unsafe { HidD_GetPreparsedData(device.0, &mut data) } == 0 {
            return Err(last_error("HidD_GetPreparsedData"));
        }
        Ok(Self(data))
    }

    /// Runs `HidP_GetCaps` against this preparsed data to get the usage
    /// page/usage and report byte lengths in one call.
    fn caps(&self) -> Result<HIDP_CAPS> {
        let mut caps = HIDP_CAPS::default();
        let status = unsafe { HidP_GetCaps(self.0, &mut caps) };
        // HidP_GetCaps reports success via a specific NTSTATUS value, not a
        // plain zero/non-zero check.
        if status != HIDP_STATUS_SUCCESS {
            return Err(Error {
                call: "HidP_GetCaps",
                code: status as u32,
            });
        }
        Ok(caps)
    }
}

impl Drop for PreparsedData {
    fn drop(&mut self) {
        unsafe { HidD_FreePreparsedData(self.0) };
    }
}

/// Decodes a NUL-terminated UTF-16 buffer, stopping at the terminator.
///
/// All the wide strings we read from Win32 (device paths, instance IDs,
/// device descriptions, HID string descriptors) come back as fixed-size
/// buffers that are NUL-terminated but not necessarily NUL-*filled* — so we
/// find the first `0` and decode only up to there, ignoring whatever
/// leftover buffer content sits after it.
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Wraps one of the `HidD_Get*String` calls; `None` when unsupported or empty.
///
/// All three (`HidD_GetManufacturerString`, `HidD_GetProductString`,
/// `HidD_GetSerialNumberString`) share the exact same calling convention, so
/// one generic helper — parameterized by which function pointer to call —
/// covers all three call sites in [`describe`].
fn wide_string_query(
    device: &File,
    query: unsafe extern "system" fn(HANDLE, *mut core::ffi::c_void, u32) -> BOOLEAN,
) -> Option<String> {
    // The HID class driver caps these strings at 4093 bytes, so a fixed
    // 4096-byte (2048 x u16) stack buffer is always large enough — no need
    // for the ask-twice sizing dance used elsewhere in this file.
    let mut buf = [0u16; 2048];
    let bytes = (buf.len() * size_of::<u16>()) as u32;
    if unsafe { query(device.0, buf.as_mut_ptr().cast(), bytes) } == 0 {
        return None;
    }
    let s = from_wide(&buf);
    if s.is_empty() { None } else { Some(s) }
}

/// Enumerates every present HID device interface.
///
/// Devices that cannot be opened are still returned, with `info` unset, so the
/// listing reflects what the system actually exposes.
///
/// High-level shape of what follows: ask `hid.dll` for the HID device
/// interface class GUID, ask SetupAPI for every present device interface of
/// that class, then walk that set one index at a time until
/// `SetupDiEnumDeviceInterfaces` reports [`ERROR_NO_MORE_ITEMS`]. For each
/// interface, resolve its path and owning device node
/// ([`interface_detail`]), then gather everything else about it
/// ([`describe`]).
pub fn enumerate() -> Result<Vec<HidDevice>> {
    let mut class = GUID::default();
    // GUID_DEVINTERFACE_HID: the one class GUID shared by every device on
    // the standard HID class driver stack, regardless of transport (USB,
    // Bluetooth, I2C-HID, or an ACPI-virtual HID device like this machine's
    // ATK4002 wireless radio control).
    unsafe { HidD_GetHidGuid(&mut class) };

    let set = DevInfoSet::present_interfaces(&class)?;
    let mut devices = Vec::new();

    // SetupDiEnumDeviceInterfaces has no "give me the count" call — the only
    // way to know how many there are is to keep incrementing the index until
    // it fails with ERROR_NO_MORE_ITEMS. This is the standard SetupAPI
    // enumeration idiom (mirrored again for strings via the ask-twice
    // sizing pattern in `interface_detail`/`instance_id`/`device_desc`).
    for index in 0.. {
        let mut iface = SP_DEVICE_INTERFACE_DATA::new();
        if unsafe { SetupDiEnumDeviceInterfaces(set.0, ptr::null_mut(), &class, index, &mut iface) }
            == 0
        {
            let err = last_error("SetupDiEnumDeviceInterfaces");
            if err.code == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(err);
        }

        let (path, mut devinfo) = interface_detail(set.0, &mut iface)?;
        devices.push(describe(&path, set.0, &mut devinfo));
    }

    Ok(devices)
}

/// Resolves a device interface to its `\\?\hid#...` path plus its devnode.
///
/// `SetupDiGetDeviceInterfaceDetailW` is called twice, the standard Win32
/// "ask twice" pattern for a variable-length result:
///
/// 1. First with a null/zero-sized output buffer. This is *expected* to
///    fail with [`ERROR_INSUFFICIENT_BUFFER`]; what we actually want out of
///    this call is the `required` byte count it reports, which tells us
///    exactly how large a buffer the real call will need (the path length
///    varies per device, so there's no fixed upper bound worth hardcoding).
/// 2. Then again with a buffer of that exact size, which actually fills in
///    the path — and, as a bonus, `devinfo`, letting one round trip recover
///    both the interface's path and a handle to its owning device node.
fn interface_detail(
    set: HDEVINFO,
    iface: &mut SP_DEVICE_INTERFACE_DATA,
) -> Result<(Vec<u16>, SP_DEVINFO_DATA)> {
    let mut required: u32 = 0;
    // Expected to fail with ERROR_INSUFFICIENT_BUFFER; we only want the size.
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            iface,
            ptr::null_mut(),
            0,
            &mut required,
            ptr::null_mut(),
        )
    };
    if required < SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE {
        return Err(last_error("SetupDiGetDeviceInterfaceDetailW"));
    }

    // A u32 buffer guarantees the 4-byte alignment the struct needs, even
    // though its logical size is in bytes (`required`) and may not be a
    // multiple of 4 — rounding up here just means the tail is unused padding.
    let mut buf = vec![0u32; (required as usize + 3) / 4];
    let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    // cbSize must be the SDK-header size (see the constant's own doc comment
    // in win32.rs), not size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() —
    // Rust's default struct layout for that type does not match what
    // setupapi.dll expects here.
    unsafe { (*detail).cbSize = SP_DEVICE_INTERFACE_DETAIL_DATA_W_CB_SIZE };

    let mut devinfo = SP_DEVINFO_DATA::new();
    if unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            iface,
            detail,
            required,
            ptr::null_mut(),
            &mut devinfo,
        )
    } == 0
    {
        return Err(last_error("SetupDiGetDeviceInterfaceDetailW"));
    }

    // DevicePath is declared as a 1-element array in SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    // but the API actually wrote a NUL-terminated string of unknown length
    // starting at that same offset, running as far as `required` allows.
    // `offset_of!` (rather than a hardcoded byte count) keeps this correct
    // if the struct's field order or padding ever changes.
    const PATH_OFFSET: usize = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let capacity = (required as usize - PATH_OFFSET) / size_of::<u16>();
    let path_ptr = unsafe { (&raw const (*detail).DevicePath).cast::<u16>() };
    let path = unsafe { std::slice::from_raw_parts(path_ptr, capacity) };

    // Copy out so the path outlives `buf` (which is about to be dropped),
    // and keep the NUL terminator since CreateFileW (via File::open_for_query)
    // expects a NUL-terminated wide string, not a length-prefixed one.
    let end = path.iter().position(|&c| c == 0).unwrap_or(capacity - 1);
    let mut owned = path[..end].to_vec();
    owned.push(0);
    Ok((owned, devinfo))
}

/// Reads a device instance ID, sized from the API's own estimate.
///
/// Same ask-twice sizing pattern as [`interface_detail`], but simpler: this
/// API writes into a plain `u16` buffer (no leading struct header to skip
/// past), so the first call's `required` count is directly the buffer length
/// to allocate for the second.
fn instance_id(set: HDEVINFO, devinfo: &mut SP_DEVINFO_DATA) -> Option<String> {
    let mut required: u32 = 0;
    unsafe { SetupDiGetDeviceInstanceIdW(set, devinfo, ptr::null_mut(), 0, &mut required) };
    if required == 0 {
        return None;
    }
    let mut buf = vec![0u16; required as usize];
    let ok = unsafe {
        SetupDiGetDeviceInstanceIdW(set, devinfo, buf.as_mut_ptr(), required, ptr::null_mut())
    };
    if ok == 0 {
        return None;
    }
    Some(from_wide(&buf))
}

/// Reads `SPDRP_DEVICEDESC`, the name Device Manager shows.
///
/// Same ask-twice sizing pattern again, but this API reports `required` in
/// *bytes* (it's a generic byte-buffer registry accessor, not
/// wide-char-aware like [`SetupDiGetDeviceInstanceIdW`]), so the buffer is
/// sized in `u16` units by dividing back down.
fn device_desc(set: HDEVINFO, devinfo: &mut SP_DEVINFO_DATA) -> Option<String> {
    let mut required: u32 = 0;
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            devinfo,
            SPDRP_DEVICEDESC,
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            &mut required,
        )
    };
    if required == 0 {
        return None;
    }
    let mut buf = vec![0u16; (required as usize + 1) / size_of::<u16>()];
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            devinfo,
            SPDRP_DEVICEDESC,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            required,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let s = from_wide(&buf);
    if s.is_empty() { None } else { Some(s) }
}

/// Collects everything we can learn about one device interface.
///
/// Split into two halves along the line that matters most for this tool:
///
/// 1. SetupAPI metadata (`path`, `instance_id`, `device_desc`) — always
///    available, since it comes from the device information set itself and
///    never requires opening the device.
/// 2. HID metadata (`info`) — requires successfully opening the interface
///    with [`File::open_for_query`] first. When that fails (most commonly
///    `ERROR_ACCESS_DENIED` on the OS's primary keyboard/mouse collections),
///    we still return a [`HidDevice`] with everything from step 1 and
///    `open_error` set, rather than dropping the device from the list —
///    "can't be queried" is itself useful information to surface.
fn describe(path: &[u16], set: HDEVINFO, devinfo: &mut SP_DEVINFO_DATA) -> HidDevice {
    let mut device = HidDevice {
        path: from_wide(path),
        instance_id: instance_id(set, devinfo),
        device_desc: device_desc(set, devinfo),
        info: None,
        open_error: None,
    };

    let file = match File::open_for_query(path) {
        Ok(file) => file,
        Err(err) => {
            device.open_error = Some(err.code);
            return device;
        }
    };

    let mut attrs = HIDD_ATTRIBUTES::new();
    if unsafe { HidD_GetAttributes(file.0, &mut attrs) } == 0 {
        device.open_error = Some(unsafe { GetLastError() });
        return device;
    }

    // Report lengths come from the parsed descriptor; a device that refuses to
    // hand it over still deserves its VID/PID listed, so fall back to zeroes
    // (HIDP_CAPS::default()) rather than failing the whole device out.
    let caps = PreparsedData::get(&file)
        .and_then(|data| data.caps())
        .unwrap_or_default();

    device.info = Some(HidInfo {
        vendor_id: attrs.VendorID,
        product_id: attrs.ProductID,
        version: attrs.VersionNumber,
        manufacturer: wide_string_query(&file, HidD_GetManufacturerString),
        product: wide_string_query(&file, HidD_GetProductString),
        serial_number: wide_string_query(&file, HidD_GetSerialNumberString),
        usage_page: caps.UsagePage,
        usage: caps.Usage,
        input_report_len: caps.InputReportByteLength,
        output_report_len: caps.OutputReportByteLength,
        feature_report_len: caps.FeatureReportByteLength,
    });
    device
}

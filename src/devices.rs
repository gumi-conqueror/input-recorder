use std::mem::size_of;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use windows::{
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                HDEVINFO, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
                SP_DEVINFO_DATA, SetupDiCreateDeviceInfoList, SetupDiDestroyDeviceInfoList,
                SetupDiGetDeviceInterfaceDetailW, SetupDiGetDevicePropertyW,
                SetupDiOpenDeviceInterfaceW,
            },
            HumanInterfaceDevice::{
                HIDD_ATTRIBUTES, HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS,
                HidD_GetAttributes, HidD_GetManufacturerString, HidD_GetProductString,
                HidD_GetSerialNumberString, HidP_Feature, HidP_GetButtonCaps, HidP_GetCaps,
                HidP_GetValueCaps, HidP_Input, HidP_Output, PHIDP_PREPARSED_DATA,
            },
            Properties::{
                DEVPKEY_Device_ContainerId, DEVPKEY_Device_DeviceDesc,
                DEVPKEY_Device_DriverVersion, DEVPKEY_Device_EnumeratorName,
                DEVPKEY_Device_FriendlyName, DEVPKEY_Device_InstanceId,
                DEVPKEY_Device_Manufacturer, DEVPROP_TYPE_GUID, DEVPROP_TYPE_STRING, DEVPROPTYPE,
            },
        },
        Foundation::{CloseHandle, DEVPROPKEY, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
        UI::Input::{
            GetRawInputDeviceInfoW, GetRawInputDeviceList, RAW_INPUT_DEVICE_INFO_COMMAND,
            RAWINPUTDEVICELIST, RID_DEVICE_INFO, RIDI_DEVICEINFO, RIDI_DEVICENAME,
            RIDI_PREPARSEDDATA, RIM_TYPEHID, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        },
    },
    core::{GUID, PCWSTR},
};

use crate::model::{Device, hex};

pub(crate) fn raw_list() -> Result<Vec<RAWINPUTDEVICELIST>> {
    for _ in 0..4 {
        let mut count = 0;
        ensure!(
            // SAFETY: the API writes only the supplied count during this size query.
            unsafe {
                GetRawInputDeviceList(None, &mut count, size_of::<RAWINPUTDEVICELIST>() as u32)
            } != u32::MAX,
            "GetRawInputDeviceList: {}",
            windows::core::Error::from_thread()
        );
        let mut devices = vec![RAWINPUTDEVICELIST::default(); count as usize];
        // SAFETY: the vector has room for count initialized device records.
        let received = unsafe {
            GetRawInputDeviceList(
                Some(devices.as_mut_ptr()),
                &mut count,
                size_of::<RAWINPUTDEVICELIST>() as u32,
            )
        };
        if received != u32::MAX {
            devices.truncate(received as usize);
            return Ok(devices);
        }
    }
    anyhow::bail!(
        "device enumeration changed repeatedly or failed: {}",
        windows::core::Error::from_thread()
    )
}

pub(crate) fn enumerate() -> Result<Vec<Device>> {
    Ok(raw_list()?.iter().map(describe).collect())
}

fn device_bytes(handle: HANDLE, command: RAW_INPUT_DEVICE_INFO_COMMAND) -> Result<Vec<usize>> {
    let mut bytes = 0;
    ensure!(
        // SAFETY: a null buffer requests the required byte count.
        unsafe { GetRawInputDeviceInfoW(Some(handle), command, None, &mut bytes) } != u32::MAX,
        "device info size: {}",
        windows::core::Error::from_thread()
    );
    ensure!(
        bytes > 0 && bytes <= 1024 * 1024,
        "invalid device info length {bytes}"
    );
    let mut buffer = vec![0_usize; (bytes as usize).div_ceil(size_of::<usize>())];
    ensure!(
        // SAFETY: the aligned buffer is at least bytes bytes long and stays alive for the call.
        unsafe {
            GetRawInputDeviceInfoW(
                Some(handle),
                command,
                Some(buffer.as_mut_ptr().cast()),
                &mut bytes,
            )
        } != u32::MAX,
        "device info: {}",
        windows::core::Error::from_thread()
    );
    Ok(buffer)
}

pub(crate) fn describe(entry: &RAWINPUTDEVICELIST) -> Device {
    let mut device = Device {
        id: format!("unknown-{:x}", entry.hDevice.0 as usize),
        path: String::new(),
        name: String::new(),
        kind: "unknown".into(),
        usage_page: 0,
        usage: 0,
        properties: json!({}),
        capabilities: Value::Null,
        errors: Vec::new(),
    };
    if let Err(error) = describe_inner(entry, &mut device) {
        device.errors.push(format!("{error:#}"));
    }
    if device.name.is_empty() {
        device.name = format!(
            "{} {:04x}:{:04x}",
            device.kind, device.usage_page, device.usage
        );
    }
    device
}

fn describe_inner(entry: &RAWINPUTDEVICELIST, device: &mut Device) -> Result<()> {
    let mut chars = 0;
    // SAFETY: both calls use a live Raw Input handle and correctly sized UTF-16 buffers.
    unsafe {
        ensure!(
            GetRawInputDeviceInfoW(Some(entry.hDevice), RIDI_DEVICENAME, None, &mut chars)
                != u32::MAX,
            "device path size: {}",
            windows::core::Error::from_thread()
        );
        ensure!(chars < 32768, "device path too long");
        let mut path = vec![0_u16; chars as usize + 1];
        ensure!(
            GetRawInputDeviceInfoW(
                Some(entry.hDevice),
                RIDI_DEVICENAME,
                Some(path.as_mut_ptr().cast()),
                &mut chars
            ) != u32::MAX,
            "device path: {}",
            windows::core::Error::from_thread()
        );
        device.path = utf16(&path);
        device.id = hex(&Sha256::digest(device.path.to_lowercase().as_bytes()));
        let mut info = RID_DEVICE_INFO {
            cbSize: size_of::<RID_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let mut bytes = info.cbSize;
        ensure!(
            GetRawInputDeviceInfoW(
                Some(entry.hDevice),
                RIDI_DEVICEINFO,
                Some((&mut info as *mut RID_DEVICE_INFO).cast()),
                &mut bytes
            ) != u32::MAX,
            "device information: {}",
            windows::core::Error::from_thread()
        );
        match info.dwType {
            RIM_TYPEKEYBOARD => {
                device.kind = "keyboard".into();
                device.usage_page = 1;
                device.usage = 6;
            }
            RIM_TYPEMOUSE => {
                device.kind = "mouse".into();
                device.usage_page = 1;
                device.usage = 2;
            }
            RIM_TYPEHID => {
                let hid = info.Anonymous.hid;
                device.kind = "hid".into();
                device.usage_page = hid.usUsagePage;
                device.usage = hid.usUsage;
                device.properties = json!({"vendor_id": hid.dwVendorId, "product_id": hid.dwProductId, "version": hid.dwVersionNumber});
            }
            _ => {}
        }
    }
    if let Err(error) = pnp_properties(device) {
        device.errors.push(format!("PnP: {error:#}"));
    }
    if let Err(error) = hid_properties(device) {
        device.errors.push(format!("HID metadata: {error:#}"));
    }
    if entry.dwType == RIM_TYPEHID {
        match hid_capabilities(entry.hDevice) {
            Ok(caps) => device.capabilities = caps,
            Err(error) => device.errors.push(format!("HID capabilities: {error:#}")),
        }
    }
    Ok(())
}

fn utf16(text: &[u16]) -> String {
    String::from_utf16_lossy(
        &text[..text
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(text.len())],
    )
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns a valid device information set.
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn pnp_properties(device: &mut Device) -> Result<()> {
    let path: Vec<u16> = device.path.encode_utf16().chain(Some(0)).collect();
    // SAFETY: all buffers remain valid for each synchronous SetupAPI call; lengths are checked.
    unsafe {
        let set = DeviceSet(SetupDiCreateDeviceInfoList(None, None)?);
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        SetupDiOpenDeviceInterfaceW(set.0, PCWSTR(path.as_ptr()), 0, Some(&mut interface))?;
        let mut required = 0;
        let _ =
            SetupDiGetDeviceInterfaceDetailW(set.0, &interface, None, 0, Some(&mut required), None);
        ensure!(
            required >= size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32
                && required < 1024 * 1024,
            "invalid interface detail size"
        );
        let mut buffer = vec![0_usize; (required as usize).div_ceil(size_of::<usize>())];
        let detail = buffer
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        SetupDiGetDeviceInterfaceDetailW(
            set.0,
            &interface,
            Some(detail),
            required,
            None,
            Some(&mut info),
        )?;
        for (name, key) in [
            ("instance_id", DEVPKEY_Device_InstanceId),
            ("container_id", DEVPKEY_Device_ContainerId),
            ("friendly_name", DEVPKEY_Device_FriendlyName),
            ("description", DEVPKEY_Device_DeviceDesc),
            ("manufacturer", DEVPKEY_Device_Manufacturer),
            ("driver_version", DEVPKEY_Device_DriverVersion),
            ("bus", DEVPKEY_Device_EnumeratorName),
        ] {
            match property(&set, &info, &key) {
                Ok(value) => {
                    device.properties[name] = json!(value);
                }
                Err(error) => {
                    device.properties[format!("{name}_unavailable")] = json!(error.to_string());
                }
            }
        }
    }
    device.name = device.properties["friendly_name"]
        .as_str()
        .or_else(|| device.properties["description"].as_str())
        .unwrap_or("")
        .into();
    Ok(())
}

fn property(set: &DeviceSet, info: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Result<String> {
    let mut kind = DEVPROPTYPE::default();
    let mut required = 0;
    // SAFETY: property size queries and reads use valid set/info references and owned buffers.
    unsafe {
        let _ =
            SetupDiGetDevicePropertyW(set.0, info, key, &mut kind, None, Some(&mut required), 0);
        ensure!(
            required > 0 && required < 1024 * 1024,
            "{}",
            windows::core::Error::from_thread()
        );
        let mut bytes = vec![0_u8; required as usize];
        SetupDiGetDevicePropertyW(set.0, info, key, &mut kind, Some(&mut bytes), None, 0)?;
        if kind == DEVPROP_TYPE_GUID && bytes.len() == size_of::<GUID>() {
            let guid = std::ptr::read_unaligned(bytes.as_ptr().cast::<GUID>());
            return Ok(format!("{guid:?}"));
        }
        ensure!(
            kind == DEVPROP_TYPE_STRING,
            "unexpected property type {}",
            kind.0
        );
        Ok(utf16(
            &bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        ))
    }
}

struct FileHandle(HANDLE);
impl Drop for FileHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns one successful CreateFileW handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn hid_properties(device: &mut Device) -> Result<()> {
    let path: Vec<u16> = device.path.encode_utf16().chain(Some(0)).collect();
    // SAFETY: zero access opens metadata only, sharing with existing device clients; buffers are owned.
    unsafe {
        let file = FileHandle(
            CreateFileW(
                PCWSTR(path.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .context("open metadata handle")?,
        );
        let mut attributes = HIDD_ATTRIBUTES {
            Size: size_of::<HIDD_ATTRIBUTES>() as u32,
            ..Default::default()
        };
        if HidD_GetAttributes(file.0, &mut attributes) {
            device.properties["vendor_id"] = json!(attributes.VendorID);
            device.properties["product_id"] = json!(attributes.ProductID);
            device.properties["version"] = json!(attributes.VersionNumber);
        }
        for (name, function) in [
            (
                "product",
                HidD_GetProductString as unsafe fn(HANDLE, *mut std::ffi::c_void, u32) -> bool,
            ),
            ("manufacturer_string", HidD_GetManufacturerString),
            ("serial_number", HidD_GetSerialNumberString),
        ] {
            let mut buffer = [0_u16; 126];
            if function(
                file.0,
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer) as u32,
            ) {
                let value = utf16(&buffer);
                if name == "product" && !value.is_empty() {
                    device.name.clone_from(&value);
                }
                device.properties[name] = json!(value);
            } else {
                device.properties[format!("{name}_unavailable")] =
                    json!(windows::core::Error::from_thread().to_string());
            }
        }
    }
    Ok(())
}

fn hid_capabilities(handle: HANDLE) -> Result<Value> {
    let data = device_bytes(handle, RIDI_PREPARSEDDATA)?;
    let preparsed = PHIDP_PREPARSED_DATA(data.as_ptr() as isize);
    let mut caps = HIDP_CAPS::default();
    // SAFETY: preparsed references live, aligned data returned by Windows for this device.
    unsafe {
        ensure!(
            HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS,
            "HidP_GetCaps failed"
        );
        let mut reports = Vec::new();
        for (name, report_type, button_count, value_count) in [
            (
                "input",
                HidP_Input,
                caps.NumberInputButtonCaps,
                caps.NumberInputValueCaps,
            ),
            (
                "output",
                HidP_Output,
                caps.NumberOutputButtonCaps,
                caps.NumberOutputValueCaps,
            ),
            (
                "feature",
                HidP_Feature,
                caps.NumberFeatureButtonCaps,
                caps.NumberFeatureValueCaps,
            ),
        ] {
            let mut buttons = vec![HIDP_BUTTON_CAPS::default(); button_count as usize];
            let mut count = button_count;
            if count > 0 {
                ensure!(
                    HidP_GetButtonCaps(report_type, buttons.as_mut_ptr(), &mut count, preparsed)
                        == HIDP_STATUS_SUCCESS,
                    "HidP_GetButtonCaps failed"
                );
            }
            for button in buttons.iter().take(count as usize) {
                let range = if button.IsRange {
                    let r = button.Anonymous.Range;
                    json!([r.UsageMin, r.UsageMax])
                } else {
                    json!([
                        button.Anonymous.NotRange.Usage,
                        button.Anonymous.NotRange.Usage
                    ])
                };
                reports.push(json!({"type": name, "kind": "button", "report_id": button.ReportID,
                    "usage_page": button.UsagePage, "usage_range": range, "link_collection": button.LinkCollection,
                    "absolute": button.IsAbsolute, "report_count": button.ReportCount}));
            }
            let mut values = vec![HIDP_VALUE_CAPS::default(); value_count as usize];
            let mut count = value_count;
            if count > 0 {
                ensure!(
                    HidP_GetValueCaps(report_type, values.as_mut_ptr(), &mut count, preparsed)
                        == HIDP_STATUS_SUCCESS,
                    "HidP_GetValueCaps failed"
                );
            }
            for value in values.iter().take(count as usize) {
                let range = if value.IsRange {
                    let r = value.Anonymous.Range;
                    json!([r.UsageMin, r.UsageMax])
                } else {
                    json!([
                        value.Anonymous.NotRange.Usage,
                        value.Anonymous.NotRange.Usage
                    ])
                };
                reports.push(json!({"type": name, "kind": "value", "report_id": value.ReportID,
                    "usage_page": value.UsagePage, "usage_range": range, "link_collection": value.LinkCollection,
                    "absolute": value.IsAbsolute, "bit_size": value.BitSize, "report_count": value.ReportCount,
                    "logical_min": value.LogicalMin, "logical_max": value.LogicalMax,
                    "physical_min": value.PhysicalMin, "physical_max": value.PhysicalMax,
                    "units": value.Units, "units_exponent": value.UnitsExp, "has_null": value.HasNull}));
            }
        }
        Ok(
            json!({"source": "windows_hid_parser", "raw_report_descriptor": null,
            "input_report_bytes": caps.InputReportByteLength, "output_report_bytes": caps.OutputReportByteLength,
            "feature_report_bytes": caps.FeatureReportByteLength, "fields": reports}),
        )
    }
}

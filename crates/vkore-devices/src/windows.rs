use super::*;
use std::{ffi::c_void, mem::size_of};
use windows::{
    Win32::{Foundation::LUID, Graphics::DXCore::*},
    core::{GUID, HRESULT, Interface},
};

// Published in Microsoft's DirectX-Headers/dxcore_interface.h; not yet generated
// by windows 0.62.2's SDK metadata. This is hardware type, not GENERIC_ML support.
const NPU: GUID = GUID::from_u128(0xd46140c4_add7_451b_9e56_06fe8c3b58ed);

pub(super) fn discover() -> Inventory {
    match enumerate() {
        Ok(result) => result,
        Err(error) => Inventory::unavailable(
            Platform::Windows,
            &format!("Windows NPU discovery is unavailable: {error}"),
        ),
    }
}
fn enumerate() -> Result<Inventory, String> {
    // SAFETY: Load only the system DXCore DLL, never a library from the working
    // directory. It is retained until every factory/list/adapter has been dropped.
    let library =
        unsafe { libloading::os::windows::Library::load_with_flags("dxcore.dll", 0x00000800) }
            .map_err(|e| e.to_string())?; // LOAD_LIBRARY_SEARCH_SYSTEM32
    type Factory = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> HRESULT;
    // SAFETY: This signature is the documented DXCoreCreateAdapterFactory ABI.
    let create = unsafe { library.get::<Factory>(b"DXCoreCreateAdapterFactory\0") }
        .map_err(|e| e.to_string())?;
    let mut raw = std::ptr::null_mut();
    // SAFETY: IID names the requested interface; raw is writable and starts null.
    unsafe { create(&IDXCoreAdapterFactory::IID, &mut raw) }
        .ok()
        .map_err(|e| e.to_string())?;
    if raw.is_null() {
        return Err("DXCore returned no factory".into());
    }
    // SAFETY: Successful COM creation transfers one owned interface reference.
    let factory = unsafe { IDXCoreAdapterFactory::from_raw(raw) };
    // SAFETY: Factory is live and GUID is the runtime-independent NPU type.
    let list: IDXCoreAdapterList =
        unsafe { factory.CreateAdapterList(&[NPU]) }.map_err(|e| e.to_string())?;
    let mut result = Inventory::empty(Platform::Windows);
    // SAFETY: list is a live COM interface owned by this thread.
    let count = unsafe { list.GetAdapterCount() };
    if count > MAX_DEVICES as u32 {
        result.warn("DXCore reports more than 64 NPUs; the list is truncated.");
    }
    for index in 0..count.min(MAX_DEVICES as u32) {
        // SAFETY: index is within the count returned by this snapshot.
        let adapter: IDXCoreAdapter = match unsafe { list.GetAdapter(index) } {
            Ok(a) => a,
            Err(e) => {
                result.warn(&format!("NPU {index}: {e}"));
                continue;
            }
        };
        match npu(&adapter) {
            Ok(npu) => result.push(npu),
            Err(error) => result.warn(&format!("NPU {index}: {error}")),
        }
    }
    Ok(result)
}
// Private typed-property boundary: callers must match the documented property
// layout to T. Only integer-only SDK POD types are used below.
unsafe fn property<T: Default>(
    adapter: &IDXCoreAdapter,
    kind: DXCoreAdapterProperty,
) -> Result<T, String> {
    let mut value = T::default();
    // SAFETY: Caller guarantees T's layout; the API receives its exact size and
    // live aligned storage. An unsuccessful query never exposes partial data.
    unsafe { adapter.GetProperty(kind, size_of::<T>(), (&mut value as *mut T).cast()) }
        .map_err(|e| e.to_string())?;
    Ok(value)
}
fn npu(adapter: &IDXCoreAdapter) -> Result<Npu, String> {
    // SAFETY: SDK defines these properties as LUID and DXCoreHardwareID.
    let luid: LUID = unsafe { property(adapter, InstanceLuid) }?;
    let hardware: DXCoreHardwareID = unsafe { property(adapter, HardwareID) }?;
    // SAFETY: Read-only size query on a live adapter.
    let size = unsafe { adapter.GetPropertySize(DriverDescription) }.map_err(|e| e.to_string())?;
    if !(1..=4096).contains(&size) {
        return Err("DXCore device name has an invalid size".into());
    }
    let mut name = vec![0u8; size];
    // SAFETY: DriverDescription is a UTF-8 byte string; name has the exact size
    // requested by GetPropertySize and is writable for the call's duration.
    unsafe { adapter.GetProperty(DriverDescription, name.len(), name.as_mut_ptr().cast()) }
        .map_err(|e| e.to_string())?;
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    let name = bounded(&String::from_utf8_lossy(&name[..end]));
    let vendor = match hardware.vendorID {
        0x8086 => Vendor::Intel,
        0x1022 | 0x1002 => Vendor::Amd,
        0x17cb | 0x5143 => Vendor::Qualcomm,
        _ => Vendor::Other,
    };
    Ok(Npu {
        id: format!("dxcore:{:08x}{:08x}", luid.HighPart as u32, luid.LowPart),
        name: if name.is_empty() {
            format!("{} NPU", vendor.label())
        } else {
            name
        },
        vendor,
        source: Source::WindowsDxCore,
        driver: None,
        core_count: None,
        pci: None,
    })
}

use super::*;
use objc2::{ClassType, msg_send, runtime::NSObjectProtocol, sel};
use objc2_core_ml::{MLModel, MLNeuralEngineComputeDevice};

pub(super) fn discover() -> Inventory {
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: NSObject's class query has this selector and BOOL return type.
        // Check availability before sending the macOS 14+ enumeration selector.
        let available: bool = unsafe {
            msg_send![MLModel::class(), respondsToSelector: sel!(availableComputeDevices)]
        };
        if !available {
            return Inventory::unavailable(
                Platform::MacOs,
                "Neural Engine discovery requires Core ML device enumeration (macOS 14 or later).",
            );
        }
        let mut inventory = Inventory::empty(Platform::MacOs);
        // SAFETY: Runtime availability checked above. This read-only query returns
        // retained device objects; none escapes this autorelease pool or thread.
        let devices = unsafe { MLModel::availableComputeDevices() };
        if devices.count() > MAX_DEVICES {
            inventory.warn("Core ML device list exceeded the discovery limit.");
        }
        for index in 0..devices.count().min(MAX_DEVICES) {
            let device = devices.objectAtIndex(index);
            if let Ok(ane) = device.downcast::<MLNeuralEngineComputeDevice>() {
                // SAFETY: Core ML exposes a read-only hardware core count; vKore
                // never mutates or shares these handles with another thread.
                // Older Core ML versions may expose the device without its count.
                let core_count = if ane.respondsToSelector(sel!(totalCoreCount)) {
                    u32::try_from(unsafe { ane.totalCoreCount() })
                        .ok()
                        .filter(|n| *n > 0)
                } else {
                    None
                };
                inventory.push(Npu {
                    id: format!("coreml:neural-engine:{index}"),
                    name: "Apple Neural Engine".into(),
                    vendor: Vendor::Apple,
                    source: Source::CoreMl,
                    driver: None,
                    core_count,
                    pci: None,
                });
            }
        }
        inventory
    })
}

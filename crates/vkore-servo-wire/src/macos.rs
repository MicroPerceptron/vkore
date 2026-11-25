//! Ownership of the public CARemoteLayer server, client and display layer.
use std::{
    ffi::{CStr, CString, c_char, c_void},
    io,
    ptr::NonNull,
};

unsafe extern "C" {
    fn vk_register_layer_server(name: *const c_char) -> i32;
    fn vk_unregister_layer_server(name: *const c_char);
    fn vk_export_layer(name: *const c_char, view: *mut c_void, id: *mut u32) -> *mut c_void;
    fn vk_end_export(client: *mut c_void);
    fn vk_import_layer(id: u32) -> *mut c_void;
    fn vk_end_import(layer: *mut c_void);
    fn vk_place_layer(layer: *mut c_void, view: *mut c_void, x: f64, y: f64, w: f64, h: f64);
}

pub struct Registration(CString);
impl Registration {
    pub fn new(name: CString) -> io::Result<Self> {
        let status = unsafe { vk_register_layer_server(name.as_ptr()) };
        if status != 0 {
            return Err(io::Error::other(format!(
                "Core Animation registration failed ({status})"
            )));
        }
        Ok(Self(name))
    }
    pub fn name(&self) -> &CStr {
        &self.0
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe { vk_unregister_layer_server(self.0.as_ptr()) };
    }
}

pub struct Export(NonNull<c_void>);
impl Export {
    /// # Safety
    /// `view` must be a live AppKit NSView on the calling main thread.
    pub unsafe fn new(name: &CStr, view: NonNull<c_void>) -> io::Result<(Self, u32)> {
        let mut id = 0;
        let obj = unsafe { vk_export_layer(name.as_ptr(), view.as_ptr(), &mut id) };
        Ok((
            Self(NonNull::new(obj).ok_or_else(|| {
                io::Error::other("Core Animation could not export the Servo surface")
            })?),
            id,
        ))
    }
}
impl Drop for Export {
    fn drop(&mut self) {
        unsafe { vk_end_export(self.0.as_ptr()) };
    }
}

pub struct Layer(NonNull<c_void>);
impl Layer {
    pub fn new(id: u32) -> io::Result<Self> {
        NonNull::new(unsafe { vk_import_layer(id) })
            .map(Self)
            .ok_or_else(|| io::Error::other("Core Animation could not import the Servo surface"))
    }
    /// # Safety
    /// `view` must be a live AppKit NSView on the calling main thread.
    pub unsafe fn place(&self, view: NonNull<c_void>, rect: [f64; 4]) {
        let [x, y, w, h] = rect;
        unsafe { vk_place_layer(self.0.as_ptr(), view.as_ptr(), x, y, w, h) }
    }
}
impl Drop for Layer {
    fn drop(&mut self) {
        unsafe { vk_end_import(self.0.as_ptr()) };
    }
}

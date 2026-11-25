//! Transfer the browser's rendered IOSurface directly to a host CALayer.
use std::{
    ffi::{CStr, CString, c_char, c_void},
    io,
    ptr::NonNull,
};

unsafe extern "C" {
    fn vk_register_layer_server(name: *const c_char) -> u32;
    fn vk_unregister_layer_server(name: *const c_char);
    fn vk_destroy_receiver(port: u32);
    fn vk_connect_export(name: *const c_char) -> u32;
    fn vk_end_export(port: u32);
    fn vk_send_frame(port: u32, w: u32, h: u32) -> i32;
    fn vk_import_layer() -> *mut c_void;
    fn vk_receive_frame(
        port: u32,
        layer: *mut c_void,
        max_w: u32,
        max_h: u32,
        max_pixels: u64,
    ) -> i32;
    fn vk_end_import(layer: *mut c_void);
    fn vk_place_layer(layer: *mut c_void, view: *mut c_void, x: f64, y: f64, w: f64, h: f64);
}

pub struct Registration {
    name: Option<CString>,
    port: u32,
}
impl Registration {
    pub fn new(name: CString) -> io::Result<Self> {
        let port = unsafe { vk_register_layer_server(name.as_ptr()) };
        if port == 0 {
            return Err(io::Error::other("Cannot register the browser GPU channel"));
        }
        Ok(Self {
            name: Some(name),
            port,
        })
    }
    pub fn unpublish(&mut self) {
        if let Some(name) = self.name.take() {
            unsafe { vk_unregister_layer_server(name.as_ptr()) };
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.unpublish();
        unsafe { vk_destroy_receiver(self.port) };
    }
}

pub struct Export(u32);
impl Export {
    pub fn connect(name: &CStr) -> io::Result<Self> {
        let port = unsafe { vk_connect_export(name.as_ptr()) };
        if port == 0 {
            return Err(io::Error::other(
                "Cannot connect to the browser GPU channel",
            ));
        }
        Ok(Self(port))
    }

    /// Snapshot the current GL read framebuffer on the render thread.
    pub fn send(&self, width: u32, height: u32) -> io::Result<()> {
        crate::check_size(width, height)?;
        let status = unsafe { vk_send_frame(self.0, width, height) };
        if status != 0 {
            return Err(io::Error::other(format!(
                "Cannot publish browser GPU surface ({status})"
            )));
        }
        Ok(())
    }
}
impl Drop for Export {
    fn drop(&mut self) {
        unsafe { vk_end_export(self.0) };
    }
}

pub struct Layer(NonNull<c_void>);
impl Layer {
    pub fn new() -> io::Result<Self> {
        NonNull::new(unsafe { vk_import_layer() })
            .map(Self)
            .ok_or_else(|| io::Error::other("Cannot create a browser display layer"))
    }
    pub fn receive(&self, receiver: &Registration) -> io::Result<()> {
        let status = unsafe {
            vk_receive_frame(
                receiver.port,
                self.0.as_ptr(),
                crate::MAX_WIDTH,
                crate::MAX_HEIGHT,
                crate::MAX_PIXELS,
            )
        };
        if status != 0 {
            return Err(io::Error::other(format!(
                "Cannot import browser GPU surface ({status})"
            )));
        }
        Ok(())
    }
    /// # Safety
    /// `view` must be a live AppKit NSView on the calling main thread.
    pub unsafe fn place(&self, view: NonNull<c_void>, rect: [f64; 4]) {
        let [x, y, w, h] = rect;
        unsafe { vk_place_layer(self.0.as_ptr(), view.as_ptr(), x, y, w, h) };
    }
}
impl Drop for Layer {
    fn drop(&mut self) {
        unsafe { vk_end_import(self.0.as_ptr()) };
    }
}

//! Export an immutable GPU snapshot. The compositor can retain it after the
//! next WebRender paint. Neither process maps the buffer into CPU memory.
use gleam::gl::{self, Gl};
use khronos_egl as egl;
use std::{
    ffi::c_void,
    io,
    os::fd::{FromRawFd, OwnedFd},
};
use vkore_servo_wire::DmaBuffer;

type Query = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut u64) -> u32;
type Export = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut i32, *mut i32, *mut i32) -> u32;
pub struct Exporter {
    api: egl::DynamicInstance<egl::EGL1_5>,
    query: Query,
    export: Export,
}
impl Exporter {
    pub fn new() -> io::Result<Self> {
        // EGL owns the current Servo display and context. Keep the library
        // loaded for the lifetime of the two checked extension entry points.
        let api = unsafe { egl::DynamicInstance::<egl::EGL1_5>::load_required() }
            .map_err(io::Error::other)?;
        let display = api
            .get_current_display()
            .ok_or_else(|| io::Error::other("No current EGL display"))?;
        let exts = api
            .query_string(Some(display), egl::EXTENSIONS)
            .map_err(io::Error::other)?;
        if !exts
            .to_bytes()
            .split(|b| *b == b' ')
            .any(|e| e == b"EGL_MESA_image_dma_buf_export")
        {
            return Err(io::Error::other(
                "The graphics driver cannot export Wayland DMA-BUF textures.",
            ));
        }
        let lookup = |name| {
            api.get_proc_address(name)
                .ok_or_else(|| io::Error::other("Missing EGL DMA-BUF entry point"))
        };
        let query = unsafe {
            std::mem::transmute::<extern "system" fn(), Query>(lookup(
                "eglExportDMABUFImageQueryMESA",
            )?)
        };
        let export = unsafe {
            std::mem::transmute::<extern "system" fn(), Export>(lookup("eglExportDMABUFImageMESA")?)
        };
        Ok(Self { api, query, export })
    }

    pub fn snapshot(
        &self,
        gl: &dyn Gl,
        width: u32,
        height: u32,
    ) -> io::Result<(OwnedFd, DmaBuffer)> {
        let display = self
            .api
            .get_current_display()
            .ok_or_else(|| io::Error::other("Missing EGL display"))?;
        let ctx = self
            .api
            .get_current_context()
            .ok_or_else(|| io::Error::other("Missing EGL context"))?;
        let texture = gl.gen_textures(1)[0];
        let mut bound = [0];
        // One integer of GL state, never a read from the page's pixel buffer.
        unsafe { gl.get_integer_v(gl::TEXTURE_BINDING_2D, &mut bound) };
        gl.bind_texture(gl::TEXTURE_2D, texture);
        gl.copy_tex_image_2d(
            gl::TEXTURE_2D,
            0,
            gl::RGBA,
            0,
            0,
            width as i32,
            height as i32,
            0,
        );
        gl.bind_texture(gl::TEXTURE_2D, bound[0] as u32);
        // An EGLImage from this texture owns the underlying allocation until
        // the exported descriptor and the compositor's import are released.
        let buffer = unsafe { egl::ClientBuffer::from_ptr(texture as usize as *mut c_void) };
        let image = self.api.create_image(
            display,
            ctx,
            egl::GL_TEXTURE_2D as u32,
            buffer,
            &[
                egl::IMAGE_PRESERVED as usize,
                egl::TRUE as usize,
                egl::ATTRIB_NONE,
            ],
        );
        let out = image.map_err(io::Error::other).and_then(|image| {
            let (mut format, mut planes) = (0, 0);
            let mut modifiers = [0_u64; 4];
            let (mut fd, mut stride, mut offset) = (-1, 0, 0);
            // The single-plane check bounds all three arrays written by EGL.
            let result = unsafe {
                if (self.query)(
                    display.as_ptr(),
                    image.as_ptr(),
                    &mut format,
                    &mut planes,
                    modifiers.as_mut_ptr(),
                ) == 0
                    || planes != 1
                {
                    Err(io::Error::other(
                        "The driver did not provide a single-plane GPU texture",
                    ))
                } else if (self.export)(
                    display.as_ptr(),
                    image.as_ptr(),
                    &mut fd,
                    &mut stride,
                    &mut offset,
                ) == 0
                    || fd < 0
                {
                    Err(io::Error::other("EGL could not export the GPU texture"))
                } else {
                    let fd = OwnedFd::from_raw_fd(fd);
                    if stride <= 0 || offset < 0 {
                        Err(io::Error::other("Invalid GPU buffer layout"))
                    } else {
                        Ok((
                            fd,
                            DmaBuffer {
                                width,
                                height,
                                format: format as u32,
                                modifier: modifiers[0],
                                stride: stride as u32,
                                offset: offset as u32,
                            },
                        ))
                    }
                }
            };
            let _ = self.api.destroy_image(display, image);
            result
        });
        gl.delete_textures(&[texture]);
        // Publish only after the producing GPU has completed the copy. No
        // compositor can race a write, even with an explicit-sync-only driver.
        if out.is_ok() {
            gl.finish();
        }
        out
    }
}

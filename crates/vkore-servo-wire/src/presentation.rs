//! Native presentation of a Servo surface in a host window.
use crate::Surface;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
use std::{io, rc::Rc};

/// Tile bounds in logical coordinates relative to the host's content area.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TileRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct Presentation {
    #[cfg(target_os = "macos")]
    registration: crate::macos::Registration,
    native: Option<Native>,
    last: Option<(Option<TileRect>, f64)>,
}

impl Presentation {
    pub fn prepare(command: &mut std::process::Command) -> io::Result<Self> {
        #[cfg(target_os = "macos")]
        let registration = {
            use std::io::Read;
            let mut nonce = [0; 16];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
            let name = format!(
                "org.microperceptron.vkore.layer.{:032x}",
                u128::from_ne_bytes(nonce)
            );
            command.env("VKORE_SURFACE_SERVER", &name);
            crate::macos::Registration::new(std::ffi::CString::new(name)?)?
        };
        let _ = command;
        Ok(Self {
            #[cfg(target_os = "macos")]
            registration,
            native: None,
            last: None,
        })
    }

    pub fn attach(&mut self, surface: Surface) -> io::Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.registration.unpublish();
        }
        self.native = Some(Native::new(surface)?);
        self.last = None;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn receive_frame(&self) -> io::Result<()> {
        self.native
            .as_ref()
            .ok_or_else(|| io::Error::other("Browser surface has not connected"))?
            .0
            .receive(&self.registration)
    }

    pub fn place(
        &mut self,
        window: &Rc<impl HasWindowHandle + HasDisplayHandle + 'static>,
        scale: f64,
        rect: Option<TileRect>,
    ) -> io::Result<()> {
        let geometry = (rect, scale);
        if self.last == Some(geometry) {
            return Ok(());
        }
        if let Some(native) = &mut self.native {
            native.place(window, scale, rect)?;
            self.last = Some(geometry);
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
struct Native(crate::macos::Layer);
#[cfg(target_os = "macos")]
impl Native {
    fn new(surface: Surface) -> io::Result<Self> {
        let Surface::MacIoSurface = surface else {
            return Err(io::Error::other("Expected a Core Animation surface"));
        };
        Ok(Self(crate::macos::Layer::new()?))
    }
    fn place(
        &self,
        window: &Rc<impl HasWindowHandle + HasDisplayHandle + 'static>,
        _scale: f64,
        rect: Option<TileRect>,
    ) -> io::Result<()> {
        let RawWindowHandle::AppKit(h) = window.window_handle().map_err(io::Error::other)?.as_raw()
        else {
            return Err(io::Error::other("Expected an AppKit view"));
        };
        let r = rect.unwrap_or_default();
        // Winit owns the NSView for the duration of this main-thread draw.
        unsafe {
            self.0.place(
                h.ns_view,
                [r.x.into(), r.y.into(), r.width.into(), r.height.into()],
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
struct X11 {
    conn: x11rb::rust_connection::RustConnection,
    child: u32,
    parent: Option<u32>,
}
#[cfg(target_os = "linux")]
impl X11 {
    fn new(surface: Surface) -> io::Result<Self> {
        let Surface::X11(child) = surface else {
            return Err(io::Error::other("Expected an X11 browser surface"));
        };
        let (conn, _) = x11rb::connect(None).map_err(io::Error::other)?;
        Ok(Self {
            conn,
            child,
            parent: None,
        })
    }
    fn place(
        &mut self,
        window: &Rc<impl HasWindowHandle + HasDisplayHandle + 'static>,
        scale: f64,
        rect: Option<TileRect>,
    ) -> io::Result<()> {
        use x11rb::{
            connection::Connection,
            protocol::{
                shape::{ConnectionExt as _, SK, SO},
                xproto::{ClipOrdering, ConfigureWindowAux, ConnectionExt as _},
            },
        };
        let parent = match window.window_handle().map_err(io::Error::other)?.as_raw() {
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).map_err(io::Error::other)?,
            RawWindowHandle::Xcb(h) => h.window.get(),
            _ => {
                return Err(io::Error::other(
                    "An X11 or XWayland window is required for the Servo surface.",
                ));
            }
        };
        if self.parent != Some(parent) {
            self.conn
                .shape_rectangles(
                    SO::SET,
                    SK::INPUT,
                    ClipOrdering::UNSORTED,
                    self.child,
                    0,
                    0,
                    &[],
                )
                .map_err(io::Error::other)?
                .check()
                .map_err(io::Error::other)?;
            self.conn
                .reparent_window(self.child, parent, 0, 0)
                .map_err(io::Error::other)?
                .check()
                .map_err(io::Error::other)?;
            self.parent = Some(parent);
        }
        if let Some(r) = rect {
            let dims = ConfigureWindowAux::new()
                .x((r.x as f64 * scale).round() as i32)
                .y((r.y as f64 * scale).round() as i32)
                .width((r.width as f64 * scale).round().max(1.) as u32)
                .height((r.height as f64 * scale).round().max(1.) as u32);
            self.conn
                .configure_window(self.child, &dims)
                .map_err(io::Error::other)?
                .check()
                .map_err(io::Error::other)?;
        }
        self.conn.flush().map_err(io::Error::other)
    }
}

#[cfg(target_os = "linux")]
enum Native {
    X11(Box<X11>),
    Wayland(Option<Box<wayland::Tile>>),
}

#[cfg(target_os = "linux")]
#[path = "wayland.rs"]
mod wayland;

#[cfg(target_os = "linux")]
impl Native {
    fn new(surface: Surface) -> io::Result<Self> {
        match surface {
            Surface::Wayland => Ok(Self::Wayland(None)),
            _ => X11::new(surface).map(Box::new).map(Self::X11),
        }
    }
    fn place(
        &mut self,
        window: &Rc<impl HasWindowHandle + HasDisplayHandle + 'static>,
        scale: f64,
        rect: Option<TileRect>,
    ) -> io::Result<()> {
        match self {
            Self::X11(x) => x.place(window, scale, rect),
            Self::Wayland(tile) => {
                if tile.is_none() {
                    *tile = Some(Box::new(wayland::Tile::new(window)?));
                }
                tile.as_mut().unwrap().place(rect)
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Presentation {
    pub fn import(&mut self, fd: std::os::fd::OwnedFd, frame: crate::DmaBuffer) -> io::Result<()> {
        match self.native.as_mut() {
            Some(Native::Wayland(Some(tile))) => tile.import(fd, frame),
            _ => Err(io::Error::other("Unexpected Wayland buffer")),
        }
    }
    pub fn poll(&mut self) -> io::Result<wayland::Progress> {
        match self.native.as_mut() {
            Some(Native::Wayland(Some(tile))) => tile.poll(),
            _ => Ok(wayland::Progress::default()),
        }
    }
}

#[cfg(windows)]
struct Native(crate::windows::Thumbnail);
#[cfg(windows)]
impl Native {
    fn new(surface: Surface) -> io::Result<Self> {
        let Surface::Win32(source) = surface else {
            return Err(io::Error::other("Expected a Windows browser surface"));
        };
        Ok(Self(crate::windows::Thumbnail::new(source)))
    }
    fn place(
        &mut self,
        window: &Rc<impl HasWindowHandle + HasDisplayHandle + 'static>,
        scale: f64,
        rect: Option<TileRect>,
    ) -> io::Result<()> {
        let RawWindowHandle::Win32(h) = window.window_handle().map_err(io::Error::other)?.as_raw()
        else {
            return Err(io::Error::other("Expected a Windows desktop window"));
        };
        let px = |v: f32| (f64::from(v) * scale).round() as i32;
        self.0.place(
            h.hwnd.get(),
            rect.map(|r| [px(r.x), px(r.y), px(r.x + r.width), px(r.y + r.height)]),
        )
    }
}

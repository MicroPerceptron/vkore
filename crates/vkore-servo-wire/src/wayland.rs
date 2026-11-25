//! Import the helper's immutable DMA-BUF as a subsurface of the iced window.
//! Use asynchronous creation so a driver rejecting a modifier cannot disconnect
//! vKore's display. The window and its display outlive this tile.
use super::TileRect;
use crate::DmaBuffer;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    rc::Rc,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{Backend, ObjectId},
    delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer, wl_compositor, wl_region, wl_registry, wl_subcompositor, wl_subsurface,
        wl_surface,
    },
};
use wayland_protocols::wp::{
    linux_dmabuf::zv1::client::{
        zwp_linux_buffer_params_v1 as params, zwp_linux_dmabuf_v1 as dmabuf,
    },
    viewporter::client::{wp_viewport, wp_viewporter},
};

#[derive(Default)]
struct Events {
    imported: Option<Result<wl_buffer::WlBuffer, &'static str>>,
    released: Vec<wl_buffer::WlBuffer>,
}
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Events {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<params::ZwpLinuxBufferParamsV1, ()> for Events {
    fn event(
        state: &mut Self,
        p: &params::ZwpLinuxBufferParamsV1,
        e: params::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.imported = Some(match e {
            params::Event::Created { buffer } => Ok(buffer),
            params::Event::Failed => Err(
                "The Wayland compositor rejected the graphics driver's DMA-BUF format or modifier.",
            ),
            _ => return,
        });
        p.destroy();
    }
    wayland_client::event_created_child!(Events, params::ZwpLinuxBufferParamsV1, [0 => (wl_buffer::WlBuffer, ())]);
}
impl Dispatch<wl_buffer::WlBuffer, ()> for Events {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        e: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(e, wl_buffer::Event::Release) {
            state.released.push(buffer.clone());
        }
    }
}
delegate_noop!(Events: ignore dmabuf::ZwpLinuxDmabufV1);
delegate_noop!(Events: ignore wl_compositor::WlCompositor);
delegate_noop!(Events: ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(Events: ignore wl_surface::WlSurface);
delegate_noop!(Events: ignore wl_region::WlRegion);
delegate_noop!(Events: ignore wl_subsurface::WlSubsurface);
delegate_noop!(Events: ignore wp_viewporter::WpViewporter);
delegate_noop!(Events: ignore wp_viewport::WpViewport);

#[derive(Default)]
pub struct Progress {
    pub pending: bool,
    pub imported: bool,
}

pub struct Tile {
    conn: Connection,
    queue: EventQueue<Events>,
    events: Events,
    dma: dmabuf::ZwpLinuxDmabufV1,
    surface: wl_surface::WlSurface,
    sub: wl_subsurface::WlSubsurface,
    viewport: wp_viewport::WpViewport,
    imported: Option<wl_buffer::WlBuffer>,
    // The compositor may still be scanning out a retired buffer. Keep at
    // most three submitted buffers and one deferred import across resizes.
    submitted: Vec<wl_buffer::WlBuffer>,
    deferred: Option<(OwnedFd, DmaBuffer)>,
    pending: bool,
    visible: bool,
    parent: wl_surface::WlSurface,
    // Drop the foreign display and surface proxies before their owning window.
    _window: Rc<dyn HasWindowHandle>,
}
impl Tile {
    pub fn new(window: &Rc<impl HasDisplayHandle + HasWindowHandle + 'static>) -> io::Result<Self> {
        let RawDisplayHandle::Wayland(d) =
            window.display_handle().map_err(io::Error::other)?.as_raw()
        else {
            return Err(io::Error::other("Expected a Wayland display"));
        };
        let RawWindowHandle::Wayland(w) =
            window.window_handle().map_err(io::Error::other)?.as_raw()
        else {
            return Err(io::Error::other("Expected a Wayland window"));
        };
        // Both pointers come from the same live Winit window. The borrowed
        // backend creates its own event queue and does not disconnect Winit.
        let conn = Connection::from_backend(unsafe {
            Backend::from_foreign_display(d.display.as_ptr().cast())
        });
        let parent_id = unsafe {
            ObjectId::from_ptr(
                wl_surface::WlSurface::interface(),
                w.surface.as_ptr().cast(),
            )
        }
        .map_err(io::Error::other)?;
        let parent = wl_surface::WlSurface::from_id(&conn, parent_id).map_err(io::Error::other)?;
        let (globals, queue) = registry_queue_init::<Events>(&conn).map_err(io::Error::other)?;
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor =
            globals.bind(&qh, 1..=4, ()).map_err(io::Error::other)?;
        let sm: wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, 1..=1, ()).map_err(io::Error::other)?;
        let vp: wp_viewporter::WpViewporter =
            globals.bind(&qh, 1..=1, ()).map_err(io::Error::other)?;
        let dma = globals.bind(&qh, 3..=3, ()).map_err(|_| {
            io::Error::other("This Wayland compositor does not support Linux DMA-BUF v3")
        })?;
        let surface = compositor.create_surface(&qh, ());
        let empty = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let sub = sm.get_subsurface(&surface, &parent, &qh, ());
        sub.set_desync();
        sm.destroy();
        let viewport = vp.get_viewport(&surface, &qh, ());
        vp.destroy();
        Ok(Self {
            conn,
            queue,
            events: Events::default(),
            dma,
            surface,
            sub,
            viewport,
            imported: None,
            submitted: Vec::new(),
            deferred: None,
            pending: false,
            visible: false,
            parent,
            _window: window.clone(),
        })
    }

    pub fn import(&mut self, fd: OwnedFd, f: DmaBuffer) -> io::Result<()> {
        crate::check_size(f.width, f.height)?;
        if self.pending || self.deferred.is_some() || f.stride == 0 {
            return Err(io::Error::other(
                "Unexpected Wayland buffer layout or request order",
            ));
        }
        if self.submitted.len() >= 3 {
            self.deferred = Some((fd, f));
            return Ok(());
        }
        self.start_import(fd, f)
    }

    fn start_import(&mut self, fd: OwnedFd, f: DmaBuffer) -> io::Result<()> {
        let p = self.dma.create_params(&self.queue.handle(), ());
        p.add(
            fd.as_fd(),
            0,
            f.offset,
            f.stride,
            (f.modifier >> 32) as u32,
            f.modifier as u32,
        );
        p.create(
            f.width as i32,
            f.height as i32,
            f.format,
            params::Flags::YInvert,
        );
        self.pending = true;
        self.conn.flush().map_err(io::Error::other)
    }

    pub fn poll(&mut self) -> io::Result<Progress> {
        self.queue
            .dispatch_pending(&mut self.events)
            .map_err(io::Error::other)?;
        for released in self.events.released.drain(..) {
            self.submitted.retain(|b| b != &released);
            if self.imported.as_ref() != Some(&released) {
                released.destroy();
            }
        }
        if self.submitted.len() < 3
            && let Some((fd, f)) = self.deferred.take()
        {
            self.start_import(fd, f)?;
        }
        if let Some(result) = self.events.imported.take() {
            self.pending = false;
            let buffer = result.map_err(io::Error::other)?;
            if let Some(old) = self.imported.replace(buffer)
                && !self.submitted.contains(&old)
            {
                old.destroy();
            }
            if self.visible {
                self.commit()?;
            }
            return Ok(Progress {
                pending: false,
                imported: true,
            });
        }
        Ok(Progress {
            pending: self.pending || self.deferred.is_some(),
            imported: false,
        })
    }
    fn commit(&mut self) -> io::Result<()> {
        if self.visible
            && let Some(buffer) = &self.imported
            && !self.submitted.contains(buffer)
        {
            self.submitted.push(buffer.clone());
        }
        self.surface.attach(
            if self.visible {
                self.imported.as_ref()
            } else {
                None
            },
            0,
            0,
        );
        self.surface.damage(0, 0, i32::MAX, i32::MAX);
        self.surface.commit();
        self.conn.flush().map_err(io::Error::other)
    }
    pub fn place(&mut self, rect: Option<TileRect>) -> io::Result<()> {
        if let Some(r) = rect {
            self.sub
                .set_position(r.x.round() as i32, r.y.round() as i32);
            self.viewport.set_destination(
                r.width.round().max(1.) as i32,
                r.height.round().max(1.) as i32,
            );
        }
        self.visible = rect.is_some();
        self.commit()?;
        // A subsurface's position takes effect at its parent's next commit,
        // even in desynchronized mode.
        self.parent.commit();
        self.conn.flush().map_err(io::Error::other)
    }
}
impl Drop for Tile {
    fn drop(&mut self) {
        if let Some(buffer) = self.imported.take()
            && !self.submitted.contains(&buffer)
        {
            buffer.destroy();
        }
        for buffer in self.submitted.drain(..) {
            buffer.destroy();
        }
        self.viewport.destroy();
        self.sub.destroy();
        self.surface.destroy();
        self.dma.destroy();
        let _ = self.conn.flush();
    }
}

//! Experimental DWM presentation. The source and destination are top-level
//! windows owned by Servo and vKore respectively. DWM does the composition.
use std::io;
use windows_sys::Win32::{Foundation::RECT, Graphics::Dwm::*};

pub struct Thumbnail {
    source: isize,
    thumbnail: isize,
}
impl Thumbnail {
    pub fn new(source: isize) -> Self {
        Self {
            source,
            thumbnail: 0,
        }
    }

    pub fn place(&mut self, destination: isize, rect: Option<[i32; 4]>) -> io::Result<()> {
        if self.thumbnail == 0 {
            // DWM checks both HWNDs and returns a handle owned by this object.
            let hr = unsafe {
                DwmRegisterThumbnail(destination as _, self.source as _, &mut self.thumbnail)
            };
            if hr < 0 {
                return Err(io::Error::other(format!(
                    "Cannot register DWM thumbnail ({hr:#x})"
                )));
            }
        }
        let [left, top, right, bottom] = rect.unwrap_or_default();
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: RECT {
                left,
                top,
                right,
                bottom,
            },
            fVisible: i32::from(rect.is_some()),
            fSourceClientAreaOnly: 1,
            ..Default::default()
        };
        let hr = unsafe { DwmUpdateThumbnailProperties(self.thumbnail, &props) };
        if hr < 0 {
            return Err(io::Error::other(format!(
                "Cannot update DWM thumbnail ({hr:#x})"
            )));
        }
        Ok(())
    }
}
impl Drop for Thumbnail {
    fn drop(&mut self) {
        if self.thumbnail != 0 {
            unsafe {
                DwmUnregisterThumbnail(self.thumbnail);
            }
        }
    }
}

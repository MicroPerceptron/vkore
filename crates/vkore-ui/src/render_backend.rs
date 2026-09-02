use axcomp::LayerScratch;
use axgfx::{Damage, Extent2d};
use axui_core::{
    Command, Frame, FrameLimits, ResourceFormat, ResourceGeneration, ResourceId, ResourceKind,
    ResourceMutability, ResourceRef,
};
use axui_raster::{
    GlyphAtlas, GlyphKey, GlyphSource, GlyphSourceError, RasterStats, ResourceStore,
    StructureRenderer, Target,
};
use cosmic_text::{
    CacheKey, CacheKeyFlags, FontSystem, SubpixelBin, SwashCache, SwashContent, fontdb,
};
use std::{alloc::System, sync::Arc, time::Instant};

const MAX_LAYERS: usize = 4;
const MAX_LAYER_PIXELS: usize = 4 * 4096 * 4096;
const GLYPH_ENTRIES: usize = 512;
const GLYPH_CELL_PIXELS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderError {
    InvalidTarget,
    ResourceSetup,
    ResourceLimit,
    UnsupportedField,
    UnsupportedResource,
    Raster(axui_raster::RasterError),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderProfile {
    pub(crate) structure_ns: u128,
    pub(crate) commands: u64,
    pub(crate) pixels_written: u64,
    pub(crate) coverage_samples: u64,
    pub(crate) glyphs: u64,
    pub(crate) atlas_hits: u64,
    pub(crate) atlas_misses: u64,
    pub(crate) scratch_high_water_pixels: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetainedProfile {
    pub(crate) submitted_frames: u64,
    pub(crate) rendered_frames: u64,
    pub(crate) unchanged_frames: u64,
}

/// vKore's CPU-linear Axiom backend.
///
/// The Iced frontend remains above `Frame<System>`. This owner keeps only
/// bounded backend state and the last accepted display list; the platform's
/// mapped XRGB buffer remains the render target and is never mirrored here.
pub(crate) struct NativeBackend {
    structure: StructureRenderer<64>,
    glyph_atlas: GlyphAtlas<System>,
    glyph_source: OpenTypeGlyphSource,
    resources: ResourceStore<System>,
    layer_scratch: LayerScratch<System>,
    accepted: Option<Frame<System>>,
    profile: RetainedProfile,
}

impl NativeBackend {
    pub(crate) fn new() -> Result<Self, RenderError> {
        let mut resources = ResourceStore::new_in(1, axui_fonts::DM_SANS.len(), System)
            .map_err(|_| RenderError::ResourceSetup)?;
        resources
            .install(ui_font(), axui_fonts::DM_SANS)
            .map_err(|_| RenderError::ResourceSetup)?;
        Ok(Self {
            structure: StructureRenderer::new(FrameLimits::NATIVE_CONTROL_PLANE),
            glyph_atlas: GlyphAtlas::new_in(
                ResourceId::new(0x564b_4f52_4541_544c).unwrap(),
                GLYPH_ENTRIES,
                GLYPH_CELL_PIXELS,
                System,
            )
            .map_err(|_| RenderError::ResourceSetup)?,
            glyph_source: OpenTypeGlyphSource::new()?,
            resources,
            layer_scratch: LayerScratch::new_in(MAX_LAYERS, MAX_LAYER_PIXELS, System)
                .map_err(|_| RenderError::ResourceSetup)?,
            accepted: None,
            profile: RetainedProfile::default(),
        })
    }

    /// Returns `false` only for an exact display list already made visible.
    pub(crate) fn begin_frame(&mut self, frame: &Frame<System>) -> bool {
        self.profile.submitted_frames += 1;
        if self
            .accepted
            .as_ref()
            .is_some_and(|accepted| accepted == frame)
        {
            self.profile.unchanged_frames += 1;
            false
        } else {
            true
        }
    }

    /// Renders a changed structure frame into the exact platform target.
    ///
    /// Fields are preflighted and refused before the target is cleared. vKore
    /// does not currently emit field materials, so admitting one without a
    /// provider would be a silent and unsafe approximation.
    pub(crate) fn render_changed(
        &mut self,
        frame: &Frame<System>,
        pixels: &mut [u32],
        extent: Extent2d,
        stride: usize,
    ) -> Result<RenderProfile, RenderError> {
        if !frame.fields().is_empty()
            || frame
                .commands()
                .iter()
                .any(|command| matches!(command, Command::DrawField(_)))
        {
            return Err(RenderError::UnsupportedField);
        }
        let builtin_font = ui_font();
        if !frame.images().is_empty()
            || frame
                .resources()
                .iter()
                .any(|resource| *resource != builtin_font)
        {
            return Err(RenderError::UnsupportedResource);
        }
        let target_len = stride
            .checked_mul(extent.height() as usize)
            .ok_or(RenderError::InvalidTarget)?;
        if stride < extent.width() as usize || pixels.len() < target_len {
            return Err(RenderError::InvalidTarget);
        }
        let layer_depth = maximum_opacity_depth(frame.commands());
        let surface_pixels = (extent.width() as usize)
            .checked_mul(extent.height() as usize)
            .ok_or(RenderError::ResourceLimit)?;
        let scratch = self
            .layer_scratch
            .prepare(surface_pixels, layer_depth)
            .map_err(|_| RenderError::ResourceLimit)?;
        pixels[..target_len].fill(0x0008_0a0d);
        let mut target = Target::new(pixels, extent.width(), extent.height(), stride)
            .map_err(|_| RenderError::InvalidTarget)?;
        let atlas_before = self.glyph_atlas.stats();
        let started = Instant::now();
        let stats = self
            .structure
            .render(
                frame.view(),
                &mut target,
                Damage::Full,
                &self.resources,
                &mut self.glyph_source,
                &mut self.glyph_atlas,
                scratch,
            )
            .map_err(RenderError::Raster)?;
        let atlas_after = self.glyph_atlas.stats();
        self.profile.rendered_frames += 1;
        Ok(render_profile(
            started.elapsed().as_nanos(),
            stats,
            atlas_after.hits.saturating_sub(atlas_before.hits),
            atlas_after.misses.saturating_sub(atlas_before.misses),
            self.layer_scratch.stats().high_water_pixels,
        ))
    }

    /// Commits the exact frame only after platform presentation succeeds.
    pub(crate) fn mark_visible(&mut self, frame: Frame<System>) {
        self.accepted = Some(frame);
    }

    #[cfg(test)]
    pub(crate) const fn retained_profile(&self) -> RetainedProfile {
        self.profile
    }

    #[cfg(test)]
    pub(crate) fn invalidate_for_measurement(&mut self) {
        self.accepted = None;
    }
}

fn render_profile(
    structure_ns: u128,
    stats: RasterStats,
    atlas_hits: u64,
    atlas_misses: u64,
    scratch_high_water_pixels: usize,
) -> RenderProfile {
    RenderProfile {
        structure_ns,
        commands: stats.commands,
        pixels_written: stats.pixels_written,
        coverage_samples: stats.coverage_samples,
        glyphs: stats.glyphs,
        atlas_hits,
        atlas_misses,
        scratch_high_water_pixels,
    }
}

fn ui_font() -> ResourceRef {
    ResourceRef {
        id: axui_iced::DEFAULT_FONT_RESOURCE,
        generation: ResourceGeneration::INITIAL,
        kind: ResourceKind::Font,
        format: ResourceFormat::Bytes,
        mutability: ResourceMutability::Immutable,
        width: 0,
        height: 0,
        row_stride: 0,
        byte_len: axui_fonts::DM_SANS.len() as u64,
    }
}

fn maximum_opacity_depth(commands: &[Command]) -> usize {
    let mut depth = 0usize;
    let mut maximum = 0usize;
    for command in commands {
        match command {
            Command::PushOpacity { .. } => {
                depth += 1;
                maximum = maximum.max(depth);
            }
            Command::PopOpacity => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    maximum
}

struct OpenTypeGlyphSource {
    fonts: FontSystem,
    face: fontdb::ID,
    swash: SwashCache,
}

impl OpenTypeGlyphSource {
    fn new() -> Result<Self, RenderError> {
        let mut fonts =
            FontSystem::new_with_fonts([fontdb::Source::Binary(Arc::new(axui_fonts::DM_SANS))]);
        fonts
            .db_mut()
            .set_sans_serif_family(axui_fonts::DM_SANS_FAMILY);
        let face = fonts
            .db()
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name(axui_fonts::DM_SANS_FAMILY)],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            })
            .ok_or(RenderError::ResourceSetup)?;
        Ok(Self {
            fonts,
            face,
            swash: SwashCache::new(),
        })
    }
}

impl GlyphSource for OpenTypeGlyphSource {
    fn rasterize(&mut self, key: GlyphKey, output: &mut [u8]) -> Result<(), GlyphSourceError> {
        if key.font != axui_iced::DEFAULT_FONT_RESOURCE || key.face_index != 0 {
            return Err(GlyphSourceError::MissingFont);
        }
        let width = usize::from(key.width);
        let height = usize::from(key.height);
        if output.len() != width * height || width == 0 || height == 0 {
            return Err(GlyphSourceError::MalformedFont);
        }
        let glyph_id = u16::try_from(key.glyph).map_err(|_| GlyphSourceError::MissingGlyph)?;
        let image = self
            .swash
            .get_image_uncached(
                &mut self.fonts,
                CacheKey {
                    font_id: self.face,
                    glyph_id,
                    font_size_bits: key.font_size_bits,
                    x_bin: subpixel_bin(key.phase_x),
                    y_bin: subpixel_bin(key.phase_y),
                    font_weight: fontdb::Weight::NORMAL,
                    flags: CacheKeyFlags::empty(),
                },
            )
            .ok_or(GlyphSourceError::MissingGlyph)?;
        if image.placement.width as usize != width || image.placement.height as usize != height {
            return Err(GlyphSourceError::MalformedFont);
        }
        match image.content {
            SwashContent::Mask => {
                if image.data.len() != output.len() {
                    return Err(GlyphSourceError::MalformedFont);
                }
                output.copy_from_slice(&image.data);
            }
            SwashContent::SubpixelMask => {
                if image.data.len() != output.len() * 3 {
                    return Err(GlyphSourceError::MalformedFont);
                }
                for (index, coverage) in output.iter_mut().enumerate() {
                    let offset = index * 3;
                    *coverage = ((u16::from(image.data[offset])
                        + u16::from(image.data[offset + 1])
                        + u16::from(image.data[offset + 2])
                        + 1)
                        / 3) as u8;
                }
            }
            SwashContent::Color => return Err(GlyphSourceError::MissingGlyph),
        }
        Ok(())
    }
}

fn subpixel_bin(phase: u8) -> SubpixelBin {
    match phase {
        0..=7 | 56..=u8::MAX => SubpixelBin::Zero,
        8..=23 => SubpixelBin::One,
        24..=39 => SubpixelBin::Two,
        _ => SubpixelBin::Three,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axui_core::{Field, Rect};

    #[test]
    fn exact_visible_frame_is_retained_without_render_work() {
        let mut backend = NativeBackend::new().unwrap();
        let frame = Frame::new_in(Rect::new(0.0, 0.0, 8.0, 8.0), 1.0, System);
        assert!(backend.begin_frame(&frame));
        let mut pixels = [0u32; 64];
        backend
            .render_changed(&frame, &mut pixels, Extent2d::new(8, 8).unwrap(), 8)
            .unwrap();
        backend.mark_visible(frame.clone());
        assert!(!backend.begin_frame(&frame));
        assert_eq!(backend.retained_profile().rendered_frames, 1);
        assert_eq!(backend.retained_profile().unchanged_frames, 1);
    }

    #[test]
    fn field_is_refused_before_target_changes() {
        let mut backend = NativeBackend::new().unwrap();
        let mut recorder = axui_core::Recorder::new_in(Rect::new(0.0, 0.0, 8.0, 8.0), 1.0, System);
        recorder.draw_field(
            Field {
                material: ResourceId::new(77).unwrap(),
                bounds: Rect::new(0.0, 0.0, 8.0, 8.0),
                input_start: 0,
                input_len: 0,
            },
            &[],
        );
        let frame = recorder.take_frame().unwrap();
        let mut pixels = [0x0012_3456; 64];
        assert_eq!(
            backend.render_changed(&frame, &mut pixels, Extent2d::new(8, 8).unwrap(), 8,),
            Err(RenderError::UnsupportedField)
        );
        assert!(pixels.iter().all(|pixel| *pixel == 0x0012_3456));
    }
}

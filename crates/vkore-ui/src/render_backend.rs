use axcomp::LayerScratch;
use axgfx::{Damage, Extent2d};
use axui_core::{
    Command, Frame, FrameLimits, ResourceFormat, ResourceGeneration, ResourceId, ResourceKind,
    ResourceMutability, ResourceRef, validate_frame,
};
use axui_raster::{
    GlyphAtlas, GlyphKey, GlyphSource, GlyphSourceError, RasterStats, ResourceData,
    ResourceProvider, ResourceStore, StructureExecution, StructureRenderer, Target,
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
    pub(crate) kernel_items: u64,
    pub(crate) scalar_items: u64,
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
    installed_fonts: Vec<ResourceRef>,
    masks: Vec<axui_iced::PublishedImage>,
    layer_scratch: LayerScratch<System>,
    accepted: Option<Frame<System>>,
    profile: RetainedProfile,
}

impl NativeBackend {
    pub(crate) fn new() -> Result<Self, RenderError> {
        Self::with_execution(StructureExecution::SharedKernels)
    }

    fn with_execution(execution: StructureExecution) -> Result<Self, RenderError> {
        // The MONOLITH faces must be registered before their resources are
        // installed; registration is what assigns each face its identity.
        crate::fonts::register().map_err(|_| RenderError::ResourceSetup)?;
        let faces = axui_iced::registered_fonts();
        let capacity =
            axui_fonts::DM_SANS.len() + faces.iter().map(|face| face.bytes.len()).sum::<usize>();
        let mut resources = ResourceStore::new_in(1 + faces.len(), capacity, System)
            .map_err(|_| RenderError::ResourceSetup)?;
        let mut installed_fonts = vec![ui_font()];
        resources
            .install(ui_font(), axui_fonts::DM_SANS)
            .map_err(|_| RenderError::ResourceSetup)?;
        for face in &faces {
            let reference = font_reference(face.resource, face.bytes.len() as u64);
            resources
                .install(reference, face.bytes)
                .map_err(|_| RenderError::ResourceSetup)?;
            installed_fonts.push(reference);
        }
        Ok(Self {
            structure: match execution {
                StructureExecution::SharedKernels => {
                    StructureRenderer::new(FrameLimits::NATIVE_CONTROL_PLANE)
                }
                StructureExecution::ScalarReference => {
                    StructureRenderer::reference(FrameLimits::NATIVE_CONTROL_PLANE)
                }
            },
            glyph_atlas: GlyphAtlas::new_in(
                ResourceId::new(0x564b_4f52_4541_544c).unwrap(),
                GLYPH_ENTRIES,
                GLYPH_CELL_PIXELS,
                System,
            )
            .map_err(|_| RenderError::ResourceSetup)?,
            glyph_source: OpenTypeGlyphSource::new()?,
            resources,
            installed_fonts,
            masks: Vec::new(),
            layer_scratch: LayerScratch::new_in(MAX_LAYERS, MAX_LAYER_PIXELS, System)
                .map_err(|_| RenderError::ResourceSetup)?,
            accepted: None,
            profile: RetainedProfile::default(),
        })
    }

    /// Accepts the coverage masks the next frame's geometry refers to.
    ///
    /// Each carries an identity of its own and lives exactly as long as the
    /// frame that names it, so these replace rather than accumulate.
    pub(crate) fn set_masks(&mut self, masks: Vec<axui_iced::PublishedImage>) {
        self.masks = masks;
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
        self.render_damaged(frame, pixels, extent, stride, Damage::Full)
    }

    fn render_damaged(
        &mut self,
        frame: &Frame<System>,
        pixels: &mut [u32],
        extent: Extent2d,
        stride: usize,
        damage: Damage<'_>,
    ) -> Result<RenderProfile, RenderError> {
        if !frame.fields().is_empty()
            || frame
                .commands()
                .iter()
                .any(|command| matches!(command, Command::DrawField(_)))
        {
            return Err(RenderError::UnsupportedField);
        }
        // Every declared resource must be a face this backend installed. The
        // first structure profile has no image path, so any image refuses.
        if frame.resources().iter().any(|resource| {
            !self.installed_fonts.contains(resource)
                && !self.masks.iter().any(|mask| mask.descriptor == *resource)
        }) {
            return Err(RenderError::UnsupportedResource);
        }
        validate_frame(frame.view(), FrameLimits::NATIVE_CONTROL_PLANE)
            .map_err(|_| RenderError::Raster(axui_raster::RasterError::InvalidFrame))?;
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
        clear_damage(pixels, extent, stride, damage)?;
        let mut target = Target::new_xrgb(pixels, extent.width(), extent.height(), stride)
            .map_err(|_| RenderError::InvalidTarget)?;
        let atlas_before = self.glyph_atlas.stats();
        let started = Instant::now();
        let stats = self
            .structure
            .render(
                frame.view(),
                &mut target,
                damage,
                &FrameResources {
                    installed: &self.resources,
                    masks: &self.masks,
                },
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

    #[cfg(test)]
    pub(crate) fn reference_for_measurement() -> Result<Self, RenderError> {
        Self::with_execution(StructureExecution::ScalarReference)
    }

    #[cfg(test)]
    pub(crate) fn render_damage_for_measurement(
        &mut self,
        frame: &Frame<System>,
        pixels: &mut [u32],
        extent: Extent2d,
        stride: usize,
        damage: Damage<'_>,
    ) -> Result<RenderProfile, RenderError> {
        self.render_damaged(frame, pixels, extent, stride, damage)
    }
}

fn clear_damage(
    pixels: &mut [u32],
    extent: Extent2d,
    stride: usize,
    damage: Damage<'_>,
) -> Result<(), RenderError> {
    let background = 0xff08_0a0d;
    match damage {
        Damage::Full => {
            let length = stride
                .checked_mul(extent.height() as usize)
                .ok_or(RenderError::InvalidTarget)?;
            pixels
                .get_mut(..length)
                .ok_or(RenderError::InvalidTarget)?
                .fill(background);
        }
        Damage::Rects(rects) => {
            if rects.is_empty() || rects.iter().any(|rect| !rect.fits(extent)) {
                return Err(RenderError::InvalidTarget);
            }
            for rect in rects {
                for y in rect.y()..rect.y() + rect.height() {
                    let start = y as usize * stride + rect.x() as usize;
                    pixels[start..start + rect.width() as usize].fill(background);
                }
            }
        }
    }
    Ok(())
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
        kernel_items: stats.kernel_items,
        scalar_items: stats.scalar_items,
    }
}

/// Resolves a frame's resources from the installed faces and its own masks.
struct FrameResources<'a> {
    installed: &'a ResourceStore<System>,
    masks: &'a [axui_iced::PublishedImage],
}

impl ResourceProvider for FrameResources<'_> {
    fn get(&self, id: ResourceId, generation: ResourceGeneration) -> Option<ResourceData<'_>> {
        if let Some(data) = self.installed.get(id, generation) {
            return Some(data);
        }
        self.masks
            .iter()
            .find(|mask| mask.descriptor.id == id && mask.descriptor.generation == generation)
            .map(|mask| ResourceData {
                descriptor: mask.descriptor,
                bytes: &mask.coverage,
            })
    }
}

fn ui_font() -> ResourceRef {
    font_reference(
        axui_iced::DEFAULT_FONT_RESOURCE,
        axui_fonts::DM_SANS.len() as u64,
    )
}

fn font_reference(id: ResourceId, byte_len: u64) -> ResourceRef {
    ResourceRef {
        id,
        generation: ResourceGeneration::INITIAL,
        kind: ResourceKind::Font,
        format: ResourceFormat::Bytes,
        mutability: ResourceMutability::Immutable,
        width: 0,
        height: 0,
        row_stride: 0,
        byte_len,
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
        + usize::from(
            commands
                .iter()
                .any(|command| matches!(command, Command::BackdropBlur { .. })),
        )
}

struct OpenTypeGlyphSource {
    fonts: FontSystem,
    faces: Vec<(ResourceId, fontdb::ID)>,
    swash: SwashCache,
}

impl OpenTypeGlyphSource {
    fn new() -> Result<Self, RenderError> {
        // This font system is the backend's own; it rasterizes coverage for
        // identities the frontend already resolved. Load the same faces so a
        // resource recorded above the boundary can be resolved below it.
        let registered = axui_iced::registered_fonts();
        let sources = core::iter::once(fontdb::Source::Binary(Arc::new(axui_fonts::DM_SANS)))
            .chain(
                registered
                    .iter()
                    .map(|face| fontdb::Source::Binary(Arc::new(face.bytes))),
            );
        let mut fonts = FontSystem::new_with_fonts(sources);
        fonts
            .db_mut()
            .set_sans_serif_family(axui_fonts::DM_SANS_FAMILY);
        let default = fonts
            .db()
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name(axui_fonts::DM_SANS_FAMILY)],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            })
            .ok_or(RenderError::ResourceSetup)?;
        let mut faces = vec![(axui_iced::DEFAULT_FONT_RESOURCE, default)];
        for face in &registered {
            // The frontend registry holds Cosmic Text ids from the shared font
            // system, so match on the face bytes this store actually loaded.
            let local = fonts
                .db()
                .faces()
                .find(|candidate| match fonts.db().face_source(candidate.id) {
                    Some((fontdb::Source::Binary(loaded), _)) => {
                        let loaded: &[u8] = loaded.as_ref().as_ref();
                        loaded.len() == face.bytes.len() && loaded == face.bytes
                    }
                    _ => false,
                })
                .map(|candidate| candidate.id)
                .ok_or(RenderError::ResourceSetup)?;
            faces.push((face.resource, local));
        }
        Ok(Self {
            fonts,
            faces,
            swash: SwashCache::new(),
        })
    }

    fn face(&self, resource: ResourceId) -> Option<fontdb::ID> {
        self.faces
            .iter()
            .find(|(id, _)| *id == resource)
            .map(|(_, face)| *face)
    }
}

impl GlyphSource for OpenTypeGlyphSource {
    fn rasterize(&mut self, key: GlyphKey, output: &mut [u8]) -> Result<(), GlyphSourceError> {
        if key.face_index != 0 {
            return Err(GlyphSourceError::MissingFont);
        }
        let face = self.face(key.font).ok_or(GlyphSourceError::MissingFont)?;
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
                    font_id: face,
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
    fn a_canvas_curve_rasterizes_through_the_real_backend() {
        use iced_core::Renderer as _;
        use iced_graphics::geometry::Renderer as _;
        use iced_graphics::geometry::frame::Backend as _;

        let mut renderer =
            axui_iced::Renderer::new_in(iced_core::renderer::Settings::default(), System);
        let bounds = iced_core::Rectangle::with_size(iced_core::Size::new(32.0, 32.0));
        renderer.reset(bounds);
        let mut frame_builder = renderer.new_frame(bounds);
        frame_builder.stroke(
            &iced_graphics::geometry::Path::new(|builder| {
                builder.move_to(iced_core::Point::new(2.0, 16.0));
                builder.bezier_curve_to(
                    iced_core::Point::new(12.0, 2.0),
                    iced_core::Point::new(20.0, 30.0),
                    iced_core::Point::new(30.0, 16.0),
                );
            }),
            iced_graphics::geometry::Stroke::default()
                .with_width(2.0)
                .with_color(iced_core::Color::from_rgb(0.81, 0.08, 0.77)),
        );
        renderer.draw_geometry(frame_builder.into_geometry());
        let frame = renderer.take_frame().unwrap();

        let mut backend = NativeBackend::new().unwrap();
        backend.set_masks(renderer.take_images());
        let mut pixels = [0xff0a_0a0c_u32; 32 * 32];
        backend
            .render_changed(&frame, &mut pixels, Extent2d::new(32, 32).unwrap(), 32)
            .unwrap();

        // Rendering clears the target first, so "changed" says nothing. Look
        // for the accent itself: red and blue high, green low.
        let inked = |pixel: u32| {
            let (r, g, b) = ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff);
            r > 0x20 && b > 0x20 && g * 2 < r
        };
        let painted: Vec<(usize, usize)> = pixels
            .iter()
            .enumerate()
            .filter(|(_, pixel)| inked(**pixel))
            .map(|(index, _)| (index % 32, index / 32))
            .collect();
        assert!(!painted.is_empty(), "the curve painted nothing");

        // A curve must ink several rows and fill none of them edge to edge --
        // exactly what an axis-aligned rectangle could never produce.
        let rows: std::collections::BTreeSet<usize> = painted.iter().map(|(_, y)| *y).collect();
        assert!(rows.len() > 4, "a curve is not a horizontal rule");
        for row in &rows {
            let width = painted.iter().filter(|(_, y)| y == row).count();
            assert!(width < 32, "row {row} was inked edge to edge");
        }
        // Its edges are antialiased, so some ink lands short of full accent.
        assert!(
            pixels
                .iter()
                .any(|pixel| inked(*pixel) && (pixel >> 16) & 0xff < 0xce),
            "expected partial coverage at the curve's edges"
        );
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

    #[test]
    fn damaged_render_clears_only_validated_xrgb_regions() {
        let mut backend = NativeBackend::new().unwrap();
        let frame = Frame::new_in(Rect::new(0.0, 0.0, 8.0, 8.0), 1.0, System);
        let extent = Extent2d::new(8, 8).unwrap();
        let damage = axgfx::PixelRect::new(2, 3, 3, 2).unwrap();
        let mut pixels = [0xff12_3456; 64];
        backend
            .render_damage_for_measurement(&frame, &mut pixels, extent, 8, Damage::Rects(&[damage]))
            .unwrap();
        for y in 0..8 {
            for x in 0..8 {
                let expected = if (2..5).contains(&x) && (3..5).contains(&y) {
                    0xff08_0a0d
                } else {
                    0xff12_3456
                };
                assert_eq!(pixels[y * 8 + x], expected);
            }
        }

        let invalid = axgfx::PixelRect::new(7, 7, 2, 1).unwrap();
        let before = pixels;
        assert_eq!(
            backend.render_damage_for_measurement(
                &frame,
                &mut pixels,
                extent,
                8,
                Damage::Rects(&[invalid]),
            ),
            Err(RenderError::InvalidTarget)
        );
        assert_eq!(pixels, before);
    }
}

//! Type-only compatibility for Iced widgets used with a caller-owned renderer.
//!
//! `iced_widget` unconditionally reexports `iced_renderer::{core, graphics}`.
//! The upstream package rejects optimized builds unless its Tiny-Skia or WGPU
//! implementation is selected, even though generic widgets need neither. This
//! crate supplies those backend-neutral reexports plus compatibility aliases
//! that select Axiom's recorder. It deliberately has no rasterizer, surface, or fallback execution
//! path.

#![forbid(unsafe_code)]
#![feature(allocator_api)]

pub use iced_core as core;
pub use iced_graphics as graphics;

/// Axiom recorder used by `iced_widget`'s generic type defaults.
pub type Renderer = axui_iced::Renderer<std::alloc::Global>;

/// Inert compositor alias retained for API compatibility.
pub type Compositor = ();

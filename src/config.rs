//! Runtime configuration, resolved from CLI args.

use crate::density::DensityMode;
use crate::renderer::RenderMode;

// -----------------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------------

/// Reference long-edge size (pixels) for r_min automatic normalization (1080p).
pub const REF_LONG_EDGE: u32 = 1920;

/// Minimum allowed r_min regardless of image size or coarseness setting.
/// Prevents the grid from becoming degenerate on tiny inputs.
pub const R_MIN_FLOOR: f32 = 0.5;

/// Default grid resolution for stippling: r_min = 1 px at 1920px long edge.
pub const REF_R_MIN_STIPPLING: f32 = 1.0;

/// Default grid resolution for Voronoi: r_min = 2 px at 1920px long edge.
pub const REF_R_MIN_VORONOI: f32 = 2.0;

/// Floor value for luminance-based density. r_max / r_min = 1 / 0.05 = 20.
pub const DENSITY_FLOOR_LUMA: f32 = 0.05;

/// FLoor value for Sobel-based density.  r_max / r_min = 1/0.15 ≈ 7.
pub const DENSITY_FLOOR_SOBEL: f32 = 0.1;

/// Maximum free retries for off-image Bridson candidates (not charged to k).
pub const MAX_OOB_RETRIES: usize = 8;

// -----------------------------------------------------------------------------
// Config
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    // Image dimensions
    pub width: u32,
    pub height: u32,

    // Sampling method
    pub method: u8,
    pub r_min: f32, // effective pixel radius, already scaled and clamped
    pub k: usize,
    pub seed: u64,

    // Density
    pub density_mode: DensityMode,
    pub blend_alpha: f32,

    // Rendering
    pub render_mode: RenderMode,

    // Output
    pub frames: usize,
    pub gif_duration: u32,
    pub gif_pause: u32,
    pub gif_scale: f32,
    pub png_scale: f32,
}

impl Config {
    /// Per-frame delay derived from total GIF duration and frame count.
    /// Clamped to 10 ms minimum (resolution limit).
    pub fn gif_delay_ms(&self) -> u32 {
        (self.gif_duration / self.frames.max(1) as u32).max(10)
    }

    /// GIF end-pause expressed as a frame-count for encoder (repeated frames).
    pub fn pause_frames(&self) -> usize {
        (self.gif_pause / self.gif_delay_ms()).max(1) as usize
    }

    /// Effective r_min based on user parameters:
    /// r_min = ref_r_min × (long_edge / REF_LONG_EDGE) × coarseness
    /// Clamped to R_MIN_FLOOR so small images never produce a degenerate grid.
    pub fn compute_r_min(
        ref_r_min: f32,
        width: u32,
        height: u32,
        coarseness: f32
    ) -> f32 {
        let scale = width.max(height) as f32 / REF_LONG_EDGE as f32;
        let r_min = ref_r_min * scale * coarseness.max(0.1);
        r_min.max(R_MIN_FLOOR)
    }
}

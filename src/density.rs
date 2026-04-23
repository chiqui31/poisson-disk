//! Density map construction and Grid structure.

use clap::ValueEnum;
use crate::config::{DENSITY_FLOOR_LUMA, DENSITY_FLOOR_SOBEL};

#[derive(Clone, Debug, ValueEnum)]
pub enum DensityMode {
    Luma,  // Inverted luminance: density ∝ darkness
    Sobel, // Sobel filter: density ∝ contours
    Blend, // Weighted blend: α·sobel + (1-α)·luma
}

// -----------------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------------

/// Compute radius map as flat Vec<f32>, indexed [y * width + x].
/// density ∈ [density_floor, 1.0]; radius = r_min / density.
pub fn build_radius_map(
    luma: &[f32],
    width: u32,
    height: u32,
    r_min: f32,
    density_mode: &DensityMode,
    blend_alpha: f32,
) -> Vec<f32> {
    // Normalize input (Percentile stretch 0.02 - 0.98) to maximize contrast
    let norm_luma = percentile_stretch(luma, 0.02, 0.98, 0.0, 1.0);

    // Generate density map
    let density: Vec<f32> = match density_mode {
        DensityMode::Luma => {
            norm_luma.iter()
                .map(|&l| lerp(DENSITY_FLOOR_LUMA, 1.0, 1.0 - l))
                .collect()
        }
        DensityMode::Sobel => {
            let raw_sobel = compute_sobel(&norm_luma, width, height);
            percentile_stretch(&raw_sobel, 0.02, 0.98, DENSITY_FLOOR_SOBEL, 1.0)
        }
        DensityMode::Blend => {
            let alpha = blend_alpha.clamp(0.0, 1.0);
            let floor = DENSITY_FLOOR_LUMA.min(DENSITY_FLOOR_SOBEL);
            let raw_sobel = compute_sobel(&norm_luma, width, height);

            // Map Sobel to [0, 1] for balanced weighting with Luma
            let norm_sobel = percentile_stretch(&raw_sobel, 0.02, 0.98, DENSITY_FLOOR_SOBEL, 1.0);

            norm_luma.iter().zip(norm_sobel.iter())
                .map(|(&l, &s)| {
                    let inv_luma = 1.0 - l;
                    let mix = lerp(inv_luma, s, alpha);
                    lerp(floor, 1.0, mix)
                })
                .collect()
        }
    };

    // Generated radius map: r = r_min / density
    density.into_iter().map(|d| r_min / d).collect()
}

const EMPTY: u32 = u32::MAX;

/// Uniform spatial grid for sampled points collision lookups.
/// Cells stored as flat Vec<f32>, indexed [y * width + x].
pub struct Grid {
    pub cell: f32,
    pub cols: usize,
    pub rows: usize,
    cells: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

impl Grid {
    pub fn new(width: u32, height: u32, r_min: f32) -> Self {
        // Cell size = r / sqrt(2) to ensure diagonal being exactly r.
        // Clamp r_min to 0.5 to avoid hyper-refined grid (memory safety).
        let cell = r_min.max(0.5) / std::f32::consts::SQRT_2;
        let cols = ((width as f32 / cell).ceil() as usize + 1).max(1);
        let rows = ((height as f32 / cell).ceil() as usize + 1).max(1);
        Grid { cell, cols, rows, cells: vec![EMPTY; cols * rows], width, height }
    }

    /// Convert cartesian coordinates to flat grid index.
    #[inline]
    fn cell_idx(&self, x: f32, y: f32) -> usize {
        let cx = ((x / self.cell).floor() as usize).min(self.cols - 1);
        let cy = ((y / self.cell).floor() as usize).min(self.rows - 1);
        cy * self.cols + cx
    }

    /// Register new point to grid.
    pub fn insert(&mut self, x: f32, y: f32, idx: usize) {
        let ci = self.cell_idx(x, y);
        debug_assert!(self.cells[ci] == EMPTY, "Grid collision at ({x},{y})");
        self.cells[ci] = idx as u32;
    }

    /// Check if new point can be placed without violating radius constraints.
    pub fn is_valid(
        &self,
        x: f32,
        y: f32,
        candidate_r: f32,
        points: &[(f32, f32)],
        radius_map: &[f32],
    ) -> bool {
        if x < 0.0 || y < 0.0 || x >= self.width as f32 || y >= self.height as f32 {
            return false;
        }

        let cx = (x / self.cell) as isize;
        let cy = (y / self.cell) as isize;

        // Dynamic search range based on radius
        let search = (candidate_r / self.cell).ceil() as isize + 1;

        for dy in -search..=search {
            for dx in -search..=search {
                let nx = cx + dx;
                let ny = cy + dy;

                if nx < 0 || ny < 0 || nx >= self.cols as isize || ny >= self.rows as isize {
                    continue;
                }

                let pi = self.cells[ny as usize * self.cols + nx as usize];
                if pi == EMPTY { continue; }

                let (px, py) = points[pi as usize];
                let d2 = sq_dist(x, y, px, py);

                // Check against both radii
                let exisiting_r = radius_at(radius_map, x, y, self.width, self.height);
                let combined_r = candidate_r.max(exisiting_r);

                if d2 < combined_r.powi(2) { return false; }
            }
        }
        true
    }
}

/// Clamped radius lookup at (x, y).
#[inline(always)]
pub fn radius_at(
    radius_map: &[f32],
    x: f32, y: f32,
    width: u32, height: u32
) -> f32 {
    let xi = (x as u32).min(width  - 1);
    let yi = (y as u32).min(height - 1);
    radius_map[(yi * width + xi) as usize]
}

// -----------------------------------------------------------------------------
// Internal helpers
// -----------------------------------------------------------------------------

/// Compute the Sobel edge magnitude of a grayscale image (flattened as 'luma').
fn compute_sobel(luma: &[f32], width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as isize, height as isize);
    let mut out = vec![0.0; luma.len()];

    // Kernel constants
    const K: [[f32; 3]; 3] = [[1., 2., 1.], [0., 0., 0.], [-1., -2., -1.]];

    for y in 0..h {
        for x in 0..w {
            let mut gx = 0.0;
            let mut gy = 0.0;

            for dy in -1..=1 {
                for dx in -1..=1 {
                    // Coordinate clamping to avoid artificial edges at borders
                    let nx = (x + dx).clamp(0, w - 1) as usize;
                    let ny = (y + dy).clamp(0, h - 1) as usize;
                    let val = luma[ny * width as usize + nx];

                    // KX = K[dx+1][dy+1]; KY = K[dy+1][dx+1]
                    gx += val * K[(dx + 1) as usize][(dy + 1) as usize];
                    gy += val * K[(dy + 1) as usize][(dx + 1) as usize];
                }
            }
            out[(y * w + x) as usize] = gx.hypot(gy);
        }
    }
    out
}

/// Linear stretch from percentile range [low_p, high_p] to [target_min, target_max].
fn percentile_stretch(
    data: &[f32],
    low_p: f32,
    high_p: f32,
    target_min: f32,
    target_max: f32
) -> Vec<f32> {
    let n = data.len();
    if n == 0 { return vec![]; }

    let mut scratch = data.to_vec();
    let li = ((n as f32 * low_p)  as usize).min(n - 1);
    let hi = ((n as f32 * high_p) as usize).min(n - 1);

    let p_low  = *scratch.select_nth_unstable_by(li, |a, b| a.total_cmp(b)).1;
    let p_high = *scratch.select_nth_unstable_by(hi, |a, b| a.total_cmp(b)).1;

    let src_range = (p_high - p_low).max(1e-5); // Prevent division by zero
    let dst_range = target_max - target_min;

    data.iter()
       .map(|&v| {
            let normalized = ((v - p_low) / src_range).clamp(0.0, 1.0);
            target_min + (normalized * dst_range)
       })
       .collect()
}

/// Standard Linear Interpolation
#[inline(always)]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Euclidean Squared Distance.
#[inline(always)]
fn sq_dist(x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    (x1 - x2).powi(2) + (y1 - y2).powi(2)
}

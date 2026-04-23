//! Frame rendering: stippling dots and Voronoi mosaic (JFA).
//! PNG saving and streaming GIF encoding.

use clap::ValueEnum;
use image::{Rgb, RgbImage, RgbaImage, Frame, imageops, Delay};
use image::codecs::gif::{GifEncoder, Repeat};
use std::path::Path;
use std::fs::File;

#[derive(Clone, Debug, ValueEnum)]
pub enum RenderMode {
    Stippling,
    Voronoi,
}

// -----------------------------------------------------------------------------
// Snapshot plan
// -----------------------------------------------------------------------------

/// Indices into the final 'points' slice that define which frames to render.
pub struct SnapPlan {
    /// counts[i] = number of points to include in frame i.
    pub counts: Vec<usize>,
    /// How many trailing frames repeat the last snapshot (end-pause).
    pub pause_frames: usize,
}

impl SnapPlan {
    /// Build snapshot plan given total number of points, middle frames
    /// and pause frames.
    pub fn build(total: usize, n_middle: usize, pause_frames: usize) -> Self {
        let mut counts = Vec::with_capacity(n_middle + 2);
        counts.push(0); // First frame: empty canvas
        for i in 1..=n_middle {
            // Evenly spaced in [1, total-1] — exclude 0 and total.
            let c = (total as f64 * i as f64 / (n_middle + 1) as f64).round() as usize;
            counts.push(c.clamp(1, total.saturating_sub(1)));
        }
        counts.push(total); // Last frame: all points (= result.png)
        counts.dedup();     // Remove any duplicates from rounding

        SnapPlan { counts, pause_frames }
    }

    /// Total number of distinct frames that will be encoded (before pause).
    pub fn n_frames(&self) -> usize { self.counts.len() }
}

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

/// Render a stippling frame: white canvas, one black pixel per point.
pub fn render_stipple(
    width: u32,
    height: u32,
    points: &[(f32, f32)]
) -> RgbImage {
    let mut out_img = RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    let black = Rgb([0, 0, 0]);
    for &(x, y) in points {
        let (xi, yi) = (x as u32, y as u32);
        if xi < width && yi < height {
            out_img.put_pixel(xi, yi, black);
        }
    }
    out_img
}

/// Render a Voronoi mosaic via the Jump Flooding Algorithm (JFA).
///
/// Complexity: O(W·H·log(max(W,H))).
/// Each cell is colored by the average RGB of original pixels in it.
pub fn render_voronoi(
    width: u32,
    height: u32,
    src_img: &RgbImage,
    points: &[(f32, f32)]
) -> RgbImage {
    // Exit for empty points sets
    if points.is_empty() {
        return RgbImage::from_pixel(width, height, Rgb([255, 255, 255]));
    }

    const NONE: u32 = u32::MAX; // JFA sentinel
    let (w, h) = (width as usize, height as usize);

    // 1. Initialize Seed Map
    // Store index of closest point found
    let mut cur = vec![NONE; w * h];
    for (pi, &(x, y)) in points.iter().enumerate() {
        let xi = (x as usize).min(w - 1);
        let yi = (y as usize).min(h - 1);
        cur[yi * w + xi] = pi as u32;
    }

    // 2. JFA Main Loop (Ping-Pong)
    // Swap src and dst buffers at each pass
    let mut next = cur.clone();
    // Initial step: largest power of 2 encompassing dimensions
    let mut step = (width.max(height) as f32).log2().ceil().exp2() as i32 / 2;

    while step >= 1 {
        for y in 0..h {
            for x in 0..w {
                let mut best_s = cur[y * w + x];
                let mut best_d = seed_dist2(points, x, y, best_s);

                // Check 8 neighbors + current pixel at current 'step' distance
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 { continue; }

                        let nx = x as i32 + dx * step;
                        let ny = y as i32 + dy * step;

                        // Bounds check for the neighbor's coordinate
                        if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                            // Get the seed index stored in that neighbor
                            let s = cur[ny as usize * w + nx as usize];
                            if s != NONE {
                                // Check if closer than current best_s + update
                                let d = seed_dist2(points, x, y, s);
                                if d < best_d { best_d = d; best_s = s; }
                            }
                        }
                    }
                }
                // Write best seed found to the 'next' buffer
                next[y * w + x] = best_s;
            }
        }
        // Swap buffers for next step
        std::mem::swap(&mut cur, &mut next);
        step /= 2;
    }

    // 3. Accumulate per-cell average color
    let np = points.len();
    let mut sums = vec![[0u64; 3]; np];
    let mut counts = vec![0u64; np];

    for (idx, pixel) in src_img.pixels().enumerate() {
        let s = cur[idx];
        if s == NONE { continue; }
        let i = s as usize;
        for c in 0..3 { sums[i][c] += pixel[c] as u64; }
        counts[i] += 1;
    }

    // 4. Paint Output Image
    let mut out_img = RgbImage::new(width, height);
    for (idx, pixel) in out_img.pixels_mut().enumerate() {
        let s = cur[idx];
        if s == NONE {
            *pixel = Rgb([255, 255, 255]) // Fallback for unclaimed pixels
        } else {
            let i = s as usize;
            let c  = counts[i].max(1); // Prevent division by zero
            *pixel = Rgb([
                (sums[i][0] / c) as u8,
                (sums[i][1] / c) as u8,
                (sums[i][2] / c) as u8
            ]);
        }
    }
    out_img
}

// -----------------------------------------------------------------------------
// Outputs
// -----------------------------------------------------------------------------

/// Save a single PNG, optionally downscaled by scale ∈ [0, 1].
pub fn save_png(img: &RgbImage, path: &Path, scale: f32) {
    let img = maybe_scale(img, scale);
    img.save(path).expect("Failed to save PNG");
}

/// Encode a GIF (streaming) by encoding frames given a snapshot plan.
pub fn save_gif<F>(
    plan: &SnapPlan,
    delay_ms: u32,
    scale: f32,
    path: &Path,
    mut make_frame: F,
) where F: FnMut(usize) -> RgbImage {
    let file = File::create(path).expect("Cannot create GIF file");
    let mut enc = GifEncoder::new_with_speed(file, 10);
    enc.set_repeat(Repeat::Infinite).unwrap();

    // Encode each frame, keeping only last frame in memory
    let mut last_frame: Option<RgbImage> = None;
    for &count in &plan.counts {
        let frame = maybe_scale(&make_frame(count), scale);
        encode_frame(&mut enc, &frame, delay_ms);
        last_frame = Some(frame);
    }

    // End-pause using last generated frame
    if let (Some(frame), p) = (last_frame, plan.pause_frames) {
        if p > 0 {
            let pause = delay_ms.saturating_mul(p as u32);
            encode_frame(&mut enc, &frame, pause);
        }
    }
}

// -----------------------------------------------------------------------------
// Internal helpers
// -----------------------------------------------------------------------------

/// Euclidean Squared Distance with fallback for empty neighbor.
#[inline(always)]
fn seed_dist2(points: &[(f32, f32)], x: usize, y: usize, s: u32) -> f32 {
    if s == u32::MAX { return f32::MAX; }
    let (sx, sy) = points[s as usize];
    (sx - x as f32).powi(2) + (sy - y as f32).powi(2)
}

/// Resize image given scale factor.
fn maybe_scale(img: &RgbImage, scale: f32) -> RgbImage {
    if (scale - 1.0).abs() < 1e-3 { return img.clone(); }
    let nw = ((img.width() as f32 * scale).round() as u32).max(1);
    let nh = ((img.height() as f32 * scale).round() as u32).max(1);
    imageops::resize(img, nw, nh, imageops::FilterType::Triangle)
}

/// Encode GIF frame.
fn encode_frame(enc: &mut GifEncoder<File>, rgb: &RgbImage, delay_ms: u32) {
    let mut rgba = RgbaImage::new(rgb.width(), rgb.height());
    for (i, j, pixel) in rgb.enumerate_pixels() {
        let Rgb([r, g, b]) = *pixel;
        rgba.put_pixel(i, j, image::Rgba([r, g, b, 255]));
    }
    let frame = Frame::from_parts(rgba, 0, 0, Delay::from_numer_denom_ms(delay_ms, 1));
    enc.encode_frame(frame).expect("GIF encode error");
}

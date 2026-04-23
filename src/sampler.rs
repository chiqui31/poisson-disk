//! Poisson-disk sampling methods

use rand::{Rng, rngs::StdRng};
use crate::config::{Config, MAX_OOB_RETRIES};
use crate::density::{Grid, radius_at};

/// Run the sampler selected by 'config.method' (1 = dart, else = Bridson).
pub fn run(config: &Config, radius: &[f32], rng: &mut StdRng) -> Vec<(f32, f32)> {
    match config.method {
        1 => dart_throwing(config, radius, rng),
        _ => bridson(config, radius, rng),
    }
}

// -----------------------------------------------------------------------------
// Method 1 — Optimised Dart Throwing
// -----------------------------------------------------------------------------
// Visits all cells in random order, tries up to k points inside each cell.

fn dart_throwing(
    config: &Config,
    radius_map: &[f32],
    rng: &mut StdRng
) -> Vec<(f32, f32)> {
    let (w, h, r_min, k) = (config.width, config.height, config.r_min, config.k);
    let mut grid = Grid::new(w, h, r_min);
    let mut points: Vec<(f32, f32)> = Vec::new();

    // Fisher-Yates shuffle over all grid cells.
    let total = grid.cols * grid.rows;
    let mut order: Vec<usize> = (0..total).collect();
    for i in (1..total).rev() {
        let j = rng.gen_range(0..=i);
        order.swap(i, j);
    }

    // Loop over cells, draw random points, store valid ones
    for cell_i in order {
        let cy = cell_i / grid.cols;
        let cx = cell_i % grid.cols;
        let x0 = cx as f32 * grid.cell;
        let y0 = cy as f32 * grid.cell;

        for _ in 0..k {
            let x = x0 + rng.r#gen::<f32>() * grid.cell;
            let y = y0 + rng.r#gen::<f32>() * grid.cell;
            if x >= w as f32 || y >= h as f32 { continue; }

            let cr = radius_at(radius_map, x, y, w, h);
            if grid.is_valid(x, y, cr, &points, radius_map) {
                grid.insert(x, y, points.len());
                points.push((x, y));
                break; // One point per cell per pass.
            }
        }
    }
    points
}

// -----------------------------------------------------------------------------
// Method 2 — Bridson's Algorithm (Modified)
// -----------------------------------------------------------------------------
// Maintains an active list of recently placed points to grow outward from.
// Point is retired from active list only when all k attempts around it fail.
//
// Key implementation details:
//   • Off-image candidates consume up to MAX_OOB_RETRIES before taking a k slot,
//     avoiding propagation death in some edge cases.
//   • Exactly one child is accepted per outer iteration (break after first hit),
//     preserving the algorithm's "uniform" spatial propagation.

fn bridson(
    config: &Config,
    radius_map: &[f32],
    rng: &mut StdRng
) -> Vec<(f32, f32)> {
    let (w, h, r_min, k) = (config.width, config.height, config.r_min, config.k);
    let mut grid = Grid::new(w, h, r_min);
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut active: Vec<usize> = Vec::new();

    // Multi-seed initialization (starting points)
    let n_seeds = 20;
    for _ in 0..n_seeds {
        let sx = rng.r#gen::<f32>() * w as f32;
        let sy = rng.r#gen::<f32>() * h as f32;
        let sr = radius_at(radius_map, sx, sy, w, h);
        // Validate seeds
        if grid.is_valid(sx, sy, sr, &points, radius_map) {
            let pi = points.len();
            grid.insert(sx, sy, pi);
            points.push((sx, sy));
            active.push(pi);
        }
    }

    while !active.is_empty() {
        let ai = rng.gen_range(0..active.len());
        let (ax, ay) = points[active[ai]];
        let ar = radius_at(radius_map, ax, ay, w, h);
        let mut found = false;

        'attempts: for _ in 0..k {
            // Sample annulus [r, 2r], retrying for free if off-image.
            let (cx, cy) = 'oob: {
                for _ in 0..=MAX_OOB_RETRIES {
                    let dist = ar * (1.0_f32 + rng.r#gen::<f32>()).sqrt();
                    let angle = rng.r#gen::<f32>() * std::f32::consts::TAU;
                    let cx = ax + angle.cos() * dist;
                    let cy = ay + angle.sin() * dist;
                    if cx >= 0.0 && cy >= 0.0 && cx < w as f32 && cy < h as f32 {
                        break 'oob (cx, cy);
                    }
                }
                continue 'attempts; // All retries off-image → consume k-slot
            };

            let cr = radius_at(radius_map, cx, cy, w, h);
            if grid.is_valid(cx, cy, cr, &points, radius_map) {
                let pi = points.len();
                grid.insert(cx, cy, pi);
                points.push((cx, cy));
                active.push(pi);
                found = true;
                break; // One child per outer iteration.
            }
        }

        if !found { active.swap_remove(ai); }
    }
    points
}

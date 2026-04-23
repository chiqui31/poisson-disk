use clap::Parser;
use image::RgbImage;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::path::{Path, PathBuf};
use std::time::Instant;

use poisson_disk::{
    config::{Config, REF_R_MIN_STIPPLING, REF_R_MIN_VORONOI},
    density::{DensityMode, build_radius_map},
    renderer::{RenderMode, SnapPlan, render_stipple, render_voronoi, save_png, save_gif},
    sampler,
};

// -----------------------------------------------------------------------------
// CLI
// -----------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name  = "poisson_disk",
    about = "Density-driven Poisson Disk sampling.\n\
             Methods : 1 = Dart Throwing, 2 = Bridson.\n\
             Density : luma | sobel | blend.\n\
             Mode    : stippling | voronoi.\n\
             Output  : result.png + result.gif.\n\n\
             r_min scales with image size relative to a 1920px reference.\n\
             default: 1px for Stippling, 2px for Voronoi.\n\
             --coarseness > 1 makes the grid coarser (fewer, larger cells)."
)]
struct Args {
    /// Input image (RGB or grayscale)
    input: PathBuf,

    /// Output directory (default: same folder as input)
    output_dir: Option<PathBuf>,

    // ── Sampling ─────────────────────────────────────────────────────────────
    /// 1 = Dart Throwing, 2 = Bridson
    #[arg(short, long, default_value_t = 2)]
    method: u8,

    /// Grid coarseness (1.0 = default cell size, 2.0 = half cell size).
    #[arg(long, default_value_t = 1.0)]
    coarseness: f32,

    /// Maximum candidate attempts per active point (Bridson) or grid cell (Dart).
    #[arg(short, long, default_value_t = 100)]
    k: usize,

    /// RNG seed for reproducible output.
    #[arg(long, default_value_t = 42)]
    seed: u64,

    // ── Density ──────────────────────────────────────────────────────────────
    /// luma | sobel | blend.
    #[arg(long, value_enum, default_value_t = DensityMode::Luma)]
    density: DensityMode,

    /// Sobel weight in blend mode (0 = pure luma, 1 = pure sobel).
    #[arg(long, default_value_t = 0.5)]
    blend_alpha: f32,

    // ── Rendering ────────────────────────────────────────────────────────────
    /// stippling | voronoi.
    #[arg(long, value_enum, default_value_t = RenderMode::Stippling)]
    render_mode: RenderMode,

    // ── Output ───────────────────────────────────────────────────────────────
    /// Number of intermediate GIF frames between empty canvas and final frame.
    #[arg(long, default_value_t = 30)]
    frames: usize,

    /// Total GIF animation duration in milliseconds (excluding end pause).
    #[arg(long, default_value_t = 6000)]
    gif_duration: u32,

    /// Duration of the end-pause in milliseconds (final frame hold time).
    #[arg(long, default_value_t = 2000)]
    gif_pause: u32,

    /// GIF frame scale (0-1).
    #[arg(long, default_value_t = 1.0)]
    gif_scale: f32,

    /// PNG image scale (0–1).
    #[arg(long, default_value_t = 1.0)]
    png_scale: f32,
}

impl Args {
    /// Resolve raw CLI values into a fully validated 'Config'.
    fn into_config(self, width: u32, height: u32) -> (Config, f32) {
        let ref_r = match self.render_mode {
            RenderMode::Stippling => REF_R_MIN_STIPPLING,
            RenderMode::Voronoi => REF_R_MIN_VORONOI,
        };
        let coarseness = self.coarseness;
        let cfg = Config {
            width,
            height,
            method: self.method,
            r_min: Config::compute_r_min(ref_r, width, height, self.coarseness),
            k: self.k,
            seed: self.seed,
            density_mode: self.density,
            blend_alpha: self.blend_alpha,
            render_mode: self.render_mode,
            frames: self.frames,
            gif_duration: self.gif_duration,
            gif_pause: self.gif_pause,
            gif_scale: self.gif_scale.clamp(0.05, 1.0),
            png_scale: self.png_scale.clamp(0.05, 1.0),
        };
        (cfg, coarseness)
    }
}

// -----------------------------------------------------------------------------
// Main
// -----------------------------------------------------------------------------

fn main() {
    let args = Args::parse();

    // ── Paths ────────────────────────────────────────────────────────────────
    let input_path = args.input.clone();
    let output_dir = args.output_dir.clone().unwrap_or_else(|| {
        input_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
    });
    std::fs::create_dir_all(&output_dir).expect("Cannot create output directory");

    // ── Load Image ───────────────────────────────────────────────────────────
    let (rgb, luma) = load_image(&input_path);
    let (w, h) = rgb.dimensions();
    println!("Image: {}  {}x{} px", input_path.display(), w, h);

    // ── Config ───────────────────────────────────────────────────────────────
    let method_name = if args.method == 1 { "Dart Throwing" } else { "Bridson" };
    let (config, coarse) = args.into_config(w, h);

    // Derived GIF timing
    let gif_delay_ms = config.gif_delay_ms();
    let pause_frames = config.pause_frames();

    println!("Mode: {:?}", config.render_mode);
    println!("Density: {:?}  r_min={:.2}px  coarseness={:.1}x",
             config.density_mode, config.r_min, coarse);
    println!("Method: {}  k={}  seed={}", method_name, config.k, config.seed);
    println!("GIF: {} frames  {}ms/frame  {}ms total + {}ms pause  scale={:.0}%",
             config.frames, gif_delay_ms, config.gif_duration, config.gif_pause,
             config.gif_scale * 100.0);

    // ── Density (radius) map ─────────────────────────────────────────────────
    let radius = build_radius_map(
        &luma, w, h, config.r_min, &config.density_mode, config.blend_alpha,
    );

    // ── Sampling ─────────────────────────────────────────────────────────────
    println!("Sampling...");
    let mut rng = StdRng::seed_from_u64(config.seed);
    let t0 = Instant::now();
    let points  = sampler::run(&config, &radius, &mut rng);
    println!("  {} points in {:.2?}", points.len(), t0.elapsed());

    // ── Snapshot plan ────────────────────────────────────────────────────────
    let plan = SnapPlan::build(points.len(), config.frames, pause_frames);
    println!("Snapshots: {} GIF frames (first + {} middle + last) + {} pause",
             plan.n_frames(), config.frames, pause_frames);

    // ── Render mode ──────────────────────────────────────────────────────────
    let render = |count: usize| -> RgbImage {
        match config.render_mode {
            RenderMode::Stippling => render_stipple(w, h, &points[..count]),
            RenderMode::Voronoi => render_voronoi(w, h, &rgb, &points[..count]),
        }
    };

    // ── PNG (final frame) ────────────────────────────────────────────────────
    let png_path = output_dir.join("result.png");
    save_png(&render(points.len()), &png_path, config.png_scale);
    println!("Saved: {}", png_path.display());

    // ── GIF (streaming) ──────────────────────────────────────────────────────
    println!("Encoding GIF...");
    let t1 = Instant::now();
    let gif_path = output_dir.join("result.gif");
    save_gif(&plan, gif_delay_ms, config.gif_scale, &gif_path, render);
    println!("  Done in {:.2?}", t1.elapsed());
    println!("Saved: {}", gif_path.display());
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Load an image from disk and compute its Rec.601 luminance map.
fn load_image(path: &Path) -> (RgbImage, Vec<f32>) {
    let rgb = image::open(path).expect("Failed to open input image").to_rgb8();
    let luma = rgb.pixels()
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    (rgb, luma)
}

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

use camwatch_motion_detection::{
    config::{InputSource, MotionDetectionConfig},
    ffmpeg::{
        FrameDimensions, StreamError, StreamOptions, VideoFrame, resolve_output_dimensions,
        stream_input_with_options,
    },
    motion::MotionDetector,
};
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum BenchmarkMode {
    Single,
    Aspect16x9,
}

#[derive(Debug, Parser)]
#[command(about = "Benchmark motion detection with separate detection and output resolutions")]
struct Args {
    #[arg(long, default_value = "tests/video")]
    fixtures: PathBuf,

    #[arg(long, value_enum, default_value_t = BenchmarkMode::Single)]
    mode: BenchmarkMode,

    #[arg(long)]
    detect_width: u32,

    #[arg(long)]
    detect_height: u32,

    #[arg(long)]
    output_width: Option<u32>,

    #[arg(long)]
    output_height: Option<u32>,

    #[arg(long, default_value_t = 5)]
    frame_rate: u32,

    #[arg(long, default_value_t = 3)]
    runs: usize,
}

#[derive(Clone, Copy, Debug)]
struct RunMetrics {
    frames: u64,
    wall_time: Duration,
    analyze_time: Duration,
}

impl RunMetrics {
    fn wall_ms_per_frame(self) -> f64 {
        duration_ms(self.wall_time) / self.frames as f64
    }

    fn analyze_ms_per_frame(self) -> f64 {
        duration_ms(self.analyze_time) / self.frames as f64
    }

    fn wall_fps(self) -> f64 {
        self.frames as f64 / self.wall_time.as_secs_f64()
    }

    fn analyze_fps(self) -> f64 {
        self.frames as f64 / self.analyze_time.as_secs_f64()
    }
}

#[derive(Clone, Debug)]
struct BenchmarkCase {
    label: String,
    settings: MotionDetectionConfig,
    output_dimensions: FrameDimensions,
}

fn main() {
    let args = Args::parse();

    let fixtures = match discover_fixtures(&args.fixtures) {
        Ok(fixtures) => fixtures,
        Err(error) => {
            eprintln!("failed to discover fixtures: {error}");
            std::process::exit(1);
        }
    };

    if fixtures.is_empty() {
        eprintln!("no .mp4 fixtures found in {}", args.fixtures.display());
        std::process::exit(1);
    }

    let cases = match build_cases(&args, &fixtures) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("failed to prepare benchmark cases: {error}");
            std::process::exit(1);
        }
    };

    for case in &cases {
        run_case(case, &fixtures, args.runs);
    }
}

fn build_cases(args: &Args, fixtures: &[PathBuf]) -> Result<Vec<BenchmarkCase>, String> {
    match args.mode {
        BenchmarkMode::Single => Ok(vec![build_single_case(
            fixtures,
            args.detect_width,
            args.detect_height,
            args.output_width,
            args.output_height,
            args.frame_rate,
            "single",
        )?]),
        BenchmarkMode::Aspect16x9 => {
            if args.output_width.is_some() || args.output_height.is_some() {
                return Err(
                    "do not pass --output-width/--output-height with --mode aspect16x9".to_owned(),
                );
            }

            let resolutions = [(320, 180), (640, 360), (1280, 720)];
            resolutions
                .into_iter()
                .map(|(output_width, output_height)| {
                    build_single_case(
                        fixtures,
                        args.detect_width,
                        args.detect_height,
                        Some(output_width),
                        Some(output_height),
                        args.frame_rate,
                        "aspect16x9",
                    )
                })
                .collect()
        }
    }
}

fn build_single_case(
    fixtures: &[PathBuf],
    detect_width: u32,
    detect_height: u32,
    output_width: Option<u32>,
    output_height: Option<u32>,
    frame_rate: u32,
    mode_label: &str,
) -> Result<BenchmarkCase, String> {
    if output_width.is_some() != output_height.is_some() {
        return Err(
            "--output-width and --output-height must either both be set or both be omitted"
                .to_owned(),
        );
    }

    let settings = MotionDetectionConfig {
        frame_width: detect_width,
        frame_height: detect_height,
        output_frame_width: output_width,
        output_frame_height: output_height,
        frame_rate,
        ..MotionDetectionConfig::default()
    };

    let reference_input = InputSource::File(fixtures[0].clone());
    let output_dimensions = resolve_output_dimensions(&reference_input, &settings)
        .map_err(|error| format!("failed to resolve output dimensions: {error}"))?;

    Ok(BenchmarkCase {
        label: format!(
            "mode={} detection={}x{} output={}x{}",
            mode_label,
            settings.frame_width,
            settings.frame_height,
            output_dimensions.width,
            output_dimensions.height,
        ),
        settings,
        output_dimensions,
    })
}

fn run_case(case: &BenchmarkCase, fixtures: &[PathBuf], runs: usize) {
    let output_pixels_per_frame =
        u64::from(case.output_dimensions.width) * u64::from(case.output_dimensions.height);
    let detect_pixels_per_frame =
        u64::from(case.settings.frame_width) * u64::from(case.settings.frame_height);

    println!(
        "benchmark {} frame_rate={} fixtures={} runs={} detect_pixels_per_frame={} output_pixels_per_frame={}",
        case.label,
        case.settings.frame_rate,
        fixtures.len(),
        runs,
        detect_pixels_per_frame,
        output_pixels_per_frame,
    );

    let mut run_results = Vec::with_capacity(runs);
    for run_index in 0..runs {
        let metrics = match benchmark_run(fixtures, &case.settings, case.output_dimensions) {
            Ok(metrics) => metrics,
            Err(error) => {
                eprintln!("benchmark run {} failed: {error}", run_index + 1);
                std::process::exit(1);
            }
        };

        println!(
            "run={} frames={} wall_ms={:.2} wall_ms_per_frame={:.4} wall_fps={:.2} analyze_ms={:.2} analyze_ms_per_frame={:.4} analyze_fps={:.2}",
            run_index + 1,
            metrics.frames,
            duration_ms(metrics.wall_time),
            metrics.wall_ms_per_frame(),
            metrics.wall_fps(),
            duration_ms(metrics.analyze_time),
            metrics.analyze_ms_per_frame(),
            metrics.analyze_fps(),
        );
        run_results.push(metrics);
    }

    let summary = median_metrics(&run_results);
    println!(
        "median frames={} wall_ms={:.2} wall_ms_per_frame={:.4} wall_fps={:.2} analyze_ms={:.2} analyze_ms_per_frame={:.4} analyze_fps={:.2} wall_output_megapixels_per_sec={:.2} analyze_detect_megapixels_per_sec={:.2}",
        summary.frames,
        duration_ms(summary.wall_time),
        summary.wall_ms_per_frame(),
        summary.wall_fps(),
        duration_ms(summary.analyze_time),
        summary.analyze_ms_per_frame(),
        summary.analyze_fps(),
        (summary.wall_fps() * output_pixels_per_frame as f64) / 1_000_000.0,
        (summary.analyze_fps() * detect_pixels_per_frame as f64) / 1_000_000.0,
    );
    println!();
}

fn benchmark_run(
    fixtures: &[PathBuf],
    settings: &MotionDetectionConfig,
    output_dimensions: FrameDimensions,
) -> Result<RunMetrics, String> {
    let mut total_frames = 0_u64;
    let mut total_analyze_time = Duration::ZERO;
    let wall_start = Instant::now();

    for fixture in fixtures {
        let (frames, analyze_time) = benchmark_fixture(fixture, settings, output_dimensions)?;
        total_frames = total_frames.saturating_add(frames);
        total_analyze_time += analyze_time;
    }

    Ok(RunMetrics {
        frames: total_frames,
        wall_time: wall_start.elapsed(),
        analyze_time: total_analyze_time,
    })
}

fn benchmark_fixture(
    fixture: &Path,
    settings: &MotionDetectionConfig,
    output_dimensions: FrameDimensions,
) -> Result<(u64, Duration), String> {
    let input = InputSource::File(fixture.to_path_buf());
    let shutdown = Arc::new(AtomicBool::new(false));
    let (frame_sender, frame_receiver) = mpsc::sync_channel::<VideoFrame>(4);
    let stream_shutdown = Arc::clone(&shutdown);

    let stream_handle = thread::spawn({
        let input = input.clone();
        let settings = settings.clone();
        move || {
            benchmark_stream(
                &input,
                &settings,
                output_dimensions,
                &frame_sender,
                &stream_shutdown,
            )
        }
    });

    let mut detector = MotionDetector::new(settings);
    let mut frame_count = 0_u64;
    let mut analyze_time = Duration::ZERO;

    for frame in frame_receiver {
        let analyze_start = Instant::now();
        let _ = detector.analyze(&frame);
        analyze_time += analyze_start.elapsed();
        frame_count = frame_count.saturating_add(1);
    }

    match stream_handle.join() {
        Ok(Ok(())) => Ok((frame_count, analyze_time)),
        Ok(Err(error)) => Err(format!("{} failed: {error}", fixture.display())),
        Err(_) => Err(format!(
            "{} caused a stream thread panic",
            fixture.display()
        )),
    }
}

fn benchmark_stream(
    input: &InputSource,
    settings: &MotionDetectionConfig,
    output_dimensions: FrameDimensions,
    frame_sender: &SyncSender<VideoFrame>,
    shutdown: &Arc<AtomicBool>,
) -> Result<(), StreamError> {
    stream_input_with_options(
        input,
        settings,
        output_dimensions,
        frame_sender,
        shutdown,
        StreamOptions {
            realtime_for_files: false,
        },
    )
}

fn discover_fixtures(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut fixtures = Vec::new();
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?;

    for entry in entries {
        let entry = entry.map_err(|error| format!("failed to read fixture entry: {error}"))?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("mp4") {
            fixtures.push(path);
        }
    }

    fixtures.sort();
    Ok(fixtures)
}

fn median_metrics(runs: &[RunMetrics]) -> RunMetrics {
    let mut ordered = runs.to_vec();
    ordered.sort_by(|left, right| {
        left.wall_ms_per_frame()
            .total_cmp(&right.wall_ms_per_frame())
    });
    ordered[ordered.len() / 2]
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::overlay::OverlayHandle;
use crate::platform::input::click_screen;
use crate::platform::win32::{self, Rect, ScreenCapture};

const STANDARD_PLAY_ZONE_HORIZONTAL_SHRINK_RATIO: f32 = 0.10;
const STANDARD_PLAY_ZONE_VERTICAL_SHRINK_RATIO: f32 = 0.05;
const BOOT_BREAKER_PLAY_ZONE_HORIZONTAL_SHRINK_RATIO: f32 = 0.15;
const BOOT_BREAKER_PLAY_ZONE_VERTICAL_SHRINK_RATIO: f32 = 0.0;

#[derive(Clone, Copy, Debug)]
pub enum LaunchProfile {
    Standard,
    BootBreaker,
}

#[derive(Clone, Copy, Debug)]
enum Detector {
    StandardFrame,
    BootBreakerFrame,
}

pub fn launch_minigame_with_log(
    overlay: &OverlayHandle,
    profile: LaunchProfile,
    log_path: &str,
) -> io::Result<Option<Rect>> {
    let mut log = LaunchLogger::open(log_path)?;

    overlay.set_lines([
        "launch: locating Dota 2 window",
        "open the minigame panel",
        "Ctrl+Shift+Q: stop",
    ]);

    let dota_rect = win32::find_window_rect("Dota 2").unwrap_or_else(win32::primary_screen_rect);
    log.write(
        "init",
        "dota_rect",
        0.0,
        None,
        Some(dota_rect),
        None,
        None,
        "source=find_window_or_primary",
    )?;
    let mut capture = ScreenCapture::new(dota_rect.width, dota_rect.height)?;

    overlay.set_lines([
        "launch: detecting minigame frame",
        "waiting for event corners",
        "Ctrl+Shift+Q: stop",
    ]);

    let launch_rect = wait_for_rect(
        overlay,
        &mut capture,
        &mut log,
        dota_rect,
        Detector::StandardFrame,
        Duration::from_secs(8),
    )?;
    let Some(launch_rect) = launch_rect else {
        return Ok(None);
    };

    if !wait_and_click_play(
        overlay,
        &mut capture,
        &mut log,
        dota_rect,
        launch_rect,
        Duration::from_secs(5),
    )? {
        return Ok(None);
    }
    sleep(Duration::from_millis(900));

    let post_detector = match profile {
        LaunchProfile::Standard => Detector::StandardFrame,
        LaunchProfile::BootBreaker => Detector::BootBreakerFrame,
    };

    let rect = wait_for_rect(
        overlay,
        &mut capture,
        &mut log,
        dota_rect,
        post_detector,
        Duration::from_secs(8),
    )?;

    let Some(rect) = rect else {
        return Ok(None);
    };

    let (horizontal_shrink, vertical_shrink) = play_zone_shrink_ratios(profile);
    let gameplay_rect = shrink_rect(rect, horizontal_shrink, vertical_shrink);

    log.write(
        "frame",
        "gameplay_rect",
        0.0,
        Some(post_detector),
        Some(dota_rect),
        Some(relative_rect(dota_rect, gameplay_rect)),
        None,
        &format!(
            "raw_rect={} play_zone_shrink_horizontal_percent={} play_zone_shrink_vertical_percent={}",
            format_rect(rect),
            (horizontal_shrink * 100.0).round() as i32,
            (vertical_shrink * 100.0).round() as i32,
        ),
    )?;
    overlay.set_lines([
        "launch: gameplay rect ready".to_string(),
        format!(
            "game rect: {},{} {}x{}",
            gameplay_rect.left, gameplay_rect.top, gameplay_rect.width, gameplay_rect.height
        ),
        "Ctrl+Shift+Q: stop".to_string(),
    ]);
    Ok(Some(gameplay_rect))
}

struct LaunchLogger {
    writer: Option<BufWriter<File>>,
    seq: u64,
}

impl LaunchLogger {
    fn open(path: &str) -> io::Result<Self> {
        if path.is_empty() {
            return Ok(Self {
                writer: None,
                seq: 0,
            });
        }

        let path = launch_log_path(path);
        let mut writer = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?,
        );
        writeln!(
            writer,
            "seq\ttime_s\tphase\tevent\tdetector\tcapture_rect\tlocal_rect\tscreen_rect\tclick_local_x\tclick_local_y\tclick_screen_x\tclick_screen_y\tmethod\tdetails"
        )?;
        Ok(Self {
            writer: Some(writer),
            seq: 0,
        })
    }

    fn write(
        &mut self,
        phase: &str,
        event: &str,
        elapsed_s: f64,
        detector: Option<Detector>,
        capture_rect: Option<Rect>,
        local_rect: Option<Rect>,
        click: Option<PlayClick>,
        details: &str,
    ) -> io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };

        self.seq += 1;
        let (local_x, local_y, screen_x, screen_y, method) = click.map_or(
            (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "",
            ),
            |click| {
                (
                    click.x.to_string(),
                    click.y.to_string(),
                    click.screen_x.to_string(),
                    click.screen_y.to_string(),
                    click.method,
                )
            },
        );
        writeln!(
            writer,
            "{}\t{:.6}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.seq,
            elapsed_s,
            phase,
            event,
            detector.map_or("", Detector::name),
            capture_rect.map_or_else(String::new, format_rect),
            local_rect.map_or_else(String::new, format_rect),
            local_rect
                .and_then(|rect| capture_rect.map(|capture| absolute_rect(capture, rect)))
                .map_or_else(String::new, format_rect),
            local_x,
            local_y,
            screen_x,
            screen_y,
            method,
            details.replace('\t', " ")
        )?;
        writer.flush()
    }
}

fn launch_log_path(path: &str) -> String {
    let source = Path::new(path);
    let mut sidecar = source.to_path_buf();
    sidecar.set_extension("launch.tsv");
    sidecar.to_string_lossy().into_owned()
}

fn format_rect(rect: Rect) -> String {
    format!("{},{},{},{}", rect.left, rect.top, rect.width, rect.height)
}

fn absolute_rect(capture_rect: Rect, local_rect: Rect) -> Rect {
    Rect {
        left: capture_rect.left + local_rect.left,
        top: capture_rect.top + local_rect.top,
        width: local_rect.width,
        height: local_rect.height,
    }
}

fn relative_rect(capture_rect: Rect, screen_rect: Rect) -> Rect {
    Rect {
        left: screen_rect.left - capture_rect.left,
        top: screen_rect.top - capture_rect.top,
        width: screen_rect.width,
        height: screen_rect.height,
    }
}

fn shrink_rect(rect: Rect, horizontal_ratio: f32, vertical_ratio: f32) -> Rect {
    let dx = (rect.width as f32 * horizontal_ratio).round() as i32;
    let dy = (rect.height as f32 * vertical_ratio).round() as i32;
    Rect {
        left: rect.left + dx,
        top: rect.top + dy,
        width: (rect.width - dx * 2).max(1),
        height: (rect.height - dy * 2).max(1),
    }
}

fn play_zone_shrink_ratios(profile: LaunchProfile) -> (f32, f32) {
    match profile {
        LaunchProfile::Standard => (
            STANDARD_PLAY_ZONE_HORIZONTAL_SHRINK_RATIO,
            STANDARD_PLAY_ZONE_VERTICAL_SHRINK_RATIO,
        ),
        LaunchProfile::BootBreaker => (
            BOOT_BREAKER_PLAY_ZONE_HORIZONTAL_SHRINK_RATIO,
            BOOT_BREAKER_PLAY_ZONE_VERTICAL_SHRINK_RATIO,
        ),
    }
}

impl Detector {
    fn name(self) -> &'static str {
        match self {
            Detector::StandardFrame => "standard_frame",
            Detector::BootBreakerFrame => "boot_breaker_frame",
        }
    }
}

fn wait_for_rect(
    overlay: &OverlayHandle,
    capture: &mut ScreenCapture,
    log: &mut LaunchLogger,
    capture_rect: Rect,
    detector: Detector,
    timeout: Duration,
) -> io::Result<Option<Rect>> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if overlay.should_stop() {
            return Ok(None);
        }

        let frame = capture.grab_bgra(capture_rect)?;
        let elapsed_s = started.elapsed().as_secs_f64();
        if let Some(local_rect) =
            detect_frame_rect(frame, capture_rect.width, capture_rect.height, detector)
        {
            let rect = absolute_rect(capture_rect, local_rect);
            log.write(
                "frame",
                "detected",
                elapsed_s,
                Some(detector),
                Some(capture_rect),
                Some(local_rect),
                None,
                "frame_rect_detected",
            )?;
            overlay.set_lines([
                "launch: frame detected".to_string(),
                format!(
                    "rect: {},{} {}x{}",
                    rect.left, rect.top, rect.width, rect.height
                ),
                "Ctrl+Shift+Q: stop".to_string(),
            ]);
            return Ok(Some(rect));
        }

        log.write(
            "frame",
            "not_found",
            elapsed_s,
            Some(detector),
            Some(capture_rect),
            None,
            None,
            "no_corner_rect",
        )?;
        overlay.set_lines([
            "launch: frame not found yet".to_string(),
            format!("elapsed: {:.1}s", started.elapsed().as_secs_f32()),
            "keep Dota 2 visible".to_string(),
            "Ctrl+Shift+Q: stop".to_string(),
        ]);
        sleep(Duration::from_millis(120));
    }

    overlay.set_lines([
        "launch failed: frame not detected",
        "check Dota 2 is visible",
        "Ctrl+Shift+Q: stop",
    ]);
    log.write(
        "frame",
        "timeout",
        started.elapsed().as_secs_f64(),
        Some(detector),
        Some(capture_rect),
        None,
        None,
        "frame_detector_timeout",
    )?;
    Ok(None)
}

fn wait_and_click_play(
    overlay: &OverlayHandle,
    capture: &mut ScreenCapture,
    log: &mut LaunchLogger,
    capture_rect: Rect,
    launch_rect: Rect,
    timeout: Duration,
) -> io::Result<bool> {
    let local_launch_rect = Rect {
        left: launch_rect.left - capture_rect.left,
        top: launch_rect.top - capture_rect.top,
        width: launch_rect.width,
        height: launch_rect.height,
    };
    let started = Instant::now();

    while started.elapsed() < timeout {
        if overlay.should_stop() {
            return Ok(false);
        }

        let frame = capture.grab_bgra(capture_rect)?;
        let elapsed_s = started.elapsed().as_secs_f64();
        if let Some(click) = detect_play_button(
            frame,
            capture_rect.width,
            capture_rect.height,
            local_launch_rect,
        ) {
            let click = click.with_screen(capture_rect);
            log.write(
                "play",
                "click",
                elapsed_s,
                None,
                Some(capture_rect),
                Some(local_launch_rect),
                Some(click),
                "before_click_screen",
            )?;
            overlay.set_lines([
                "launch: PLAY detected".to_string(),
                format!("click: {},{}", click.screen_x, click.screen_y),
                "redetecting game area after start".to_string(),
                "Ctrl+Shift+Q: stop".to_string(),
            ]);
            let actual = click_screen(click.screen_x, click.screen_y)?;
            let actual_click = PlayClick {
                x: actual.x - capture_rect.left,
                y: actual.y - capture_rect.top,
                screen_x: actual.x,
                screen_y: actual.y,
                method: click.method,
            };
            overlay.mark_click(actual_click.screen_x, actual_click.screen_y);
            log.write(
                "play",
                "clicked",
                elapsed_s,
                None,
                Some(capture_rect),
                Some(local_launch_rect),
                Some(actual_click),
                "after_click_screen_actual_cursor",
            )?;
            return Ok(true);
        }

        log.write(
            "play",
            "not_found",
            elapsed_s,
            None,
            Some(capture_rect),
            Some(local_launch_rect),
            None,
            "play_detector_no_match",
        )?;
        overlay.set_lines([
            "launch: PLAY button not found yet".to_string(),
            format!("elapsed: {:.1}s", started.elapsed().as_secs_f32()),
            "keep minigame panel visible".to_string(),
            "Ctrl+Shift+Q: stop".to_string(),
        ]);
        sleep(Duration::from_millis(100));
    }

    overlay.set_lines([
        "launch failed: PLAY not detected",
        "check minigame panel is visible",
        "Ctrl+Shift+Q: stop",
    ]);
    log.write(
        "play",
        "timeout",
        started.elapsed().as_secs_f64(),
        None,
        Some(capture_rect),
        Some(local_launch_rect),
        None,
        "play_detector_timeout",
    )?;
    Ok(false)
}

fn detect_frame_rect(frame: &[u8], width: i32, height: i32, detector: Detector) -> Option<Rect> {
    let x0 = width / 10;
    let x1 = width - width / 10;
    let y0 = height / 12;
    let y1 = height - height / 20;

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    let mut count = 0i32;

    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * width + x) * 4) as usize;
            let b = frame[i];
            let g = frame[i + 1];
            let r = frame[i + 2];

            let hit = match detector {
                Detector::StandardFrame => standard_corner_pixel(r, g, b),
                Detector::BootBreakerFrame => boot_breaker_corner_pixel(r, g, b),
            };

            if hit {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                count += 1;
            }
        }
    }

    if count < 120 {
        return None;
    }

    let (left_pad, top_pad, right_pad, bottom_pad) = match detector {
        Detector::StandardFrame => (42, 24, 42, 36),
        Detector::BootBreakerFrame => (4, 4, 10, 28),
    };

    let left = (min_x - left_pad).max(0);
    let top = (min_y - top_pad).max(0);
    let right = (max_x + right_pad).min(width - 1);
    let bottom = (max_y + bottom_pad).min(height - 1);
    let rect = Rect {
        left,
        top,
        width: right - left,
        height: bottom - top,
    };

    if rect.width < 220 || rect.height < 160 {
        return None;
    }

    Some(rect)
}

fn standard_corner_pixel(r: u8, g: u8, b: u8) -> bool {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);

    (55..=190).contains(&r)
        && (45..=155).contains(&g)
        && (35..=135).contains(&b)
        && max - min <= 90
        && r >= b
}

fn boot_breaker_corner_pixel(r: u8, g: u8, b: u8) -> bool {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;

    (r >= 115 && g >= 20 && g <= 175 && b <= 90) || (r >= 70 && g <= 70 && b <= 90 && r > b + 25)
}

#[derive(Clone, Copy)]
struct PlayClick {
    x: i32,
    y: i32,
    screen_x: i32,
    screen_y: i32,
    method: &'static str,
}

impl PlayClick {
    fn local(x: i32, y: i32, method: &'static str) -> Self {
        Self {
            x,
            y,
            screen_x: 0,
            screen_y: 0,
            method,
        }
    }

    fn with_screen(mut self, capture_rect: Rect) -> Self {
        self.screen_x = capture_rect.left + self.x;
        self.screen_y = capture_rect.top + self.y;
        self
    }
}

fn detect_play_button(frame: &[u8], width: i32, height: i32, rect: Rect) -> Option<PlayClick> {
    let x0 = (rect.left + rect.width * 28 / 100).clamp(0, width - 1);
    let x1 = (rect.left + rect.width * 72 / 100).clamp(0, width - 1);

    let y0 = (rect.top + rect.height * 80 / 100).clamp(0, height - 1);
    let y1 = (rect.top + rect.height * 95 / 100).clamp(0, height - 1);

    if let Some((x, y)) = detect_play_by_gold_frame_inner(frame, width, x0, y0, x1, y1) {
        return Some(PlayClick::local(x, y, "play_gold_frame"));
    }

    let x = rect.left + rect.width / 2;
    let y = rect.top + rect.height * 87 / 100;

    Some(PlayClick::local(x, y, "play_anchor"))
}

fn detect_play_by_gold_frame_inner(
    frame: &[u8],
    width: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
) -> Option<(i32, i32)> {
    if x0 >= x1 || y0 >= y1 {
        return None;
    }

    let expected_x = (x0 + x1) / 2;
    let max_center_error = ((x1 - x0) / 6).clamp(50, 90);

    for y in y0..=y1 {
        let Some((min_x, max_x, count)) = gold_row_span(frame, width, x0, x1, y) else {
            continue;
        };
        if count < 40 {
            continue;
        }

        let span = max_x.saturating_sub(min_x);

        if !(90..=380).contains(&span) {
            continue;
        }

        let center_x = (min_x + max_x) / 2;

        if (center_x - expected_x).abs() > max_center_error {
            continue;
        }

        let bottom_y = find_play_button_bottom_edge(
            frame,
            width,
            x0,
            x1,
            y,
            y1,
            center_x,
            span,
            max_center_error,
        );
        let click_y = bottom_y
            .map(|bottom| (y + bottom) / 2)
            .unwrap_or_else(|| (y + (span / 6).clamp(28, 44)).min(y1));

        return Some((center_x, click_y));
    }

    None
}

fn find_play_button_bottom_edge(
    frame: &[u8],
    width: i32,
    x0: i32,
    x1: i32,
    top_y: i32,
    y1: i32,
    top_center_x: i32,
    top_span: i32,
    max_center_error: i32,
) -> Option<i32> {
    let mut bottom_y = None;
    let min_span = top_span * 65 / 100;
    let max_span = top_span * 125 / 100;
    let scan_bottom = (top_y + 90).min(y1);

    for y in (top_y + 12)..=scan_bottom {
        let Some((min_x, max_x, count)) = gold_row_span(frame, width, x0, x1, y) else {
            continue;
        };
        if count < 24 {
            continue;
        }

        let span = max_x.saturating_sub(min_x);
        if span < min_span || span > max_span {
            continue;
        }

        let center_x = (min_x + max_x) / 2;
        if (center_x - top_center_x).abs() > max_center_error {
            continue;
        }

        bottom_y = Some(y);
    }

    bottom_y
}

fn gold_row_span(frame: &[u8], width: i32, x0: i32, x1: i32, y: i32) -> Option<(i32, i32, i32)> {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut count = 0i32;

    for x in x0..=x1 {
        let i = ((y * width + x) * 4) as usize;

        let b = frame[i];
        let g = frame[i + 1];
        let r = frame[i + 2];

        if play_button_gold_pixel(r, g, b) {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            count += 1;
        }
    }

    (count > 0).then_some((min_x, max_x, count))
}

fn play_button_gold_pixel(r: u8, g: u8, b: u8) -> bool {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;

    (95..=235).contains(&r)
        && (70..=190).contains(&g)
        && (35..=145).contains(&b)
        && r >= g
        && g >= b - 18
        && r - b >= 25
}

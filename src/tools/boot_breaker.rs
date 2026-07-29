use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use crate::overlay::{self, OverlayHandle};
use crate::platform::input::{HeldKey, hold_key, key_event, release_keys, tap_space};
#[cfg(debug_assertions)]
use crate::platform::overlay::OverlayPrimitive;
use crate::platform::win32::{self, Bool, Dword, ScreenCapture, VK_A, VK_D, VK_SPACE};
use crate::tools::launcher::{LaunchProfile, launch_minigame_with_log};
use crate::tools::{Tool, ToolResult};

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn console_ctrl_handler(_ctrl_type: Dword) -> Bool {
    STOP.store(true, Ordering::SeqCst);
    1
}
#[derive(Clone, Copy, Debug)]
struct Roi {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug)]
struct BootSize {
    w: i32,
    h: i32,
}

#[derive(Clone, Copy, Debug)]
struct Detection {
    x: i32,
    y: i32,
    score: f32,
    bbox: Roi,
    source: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct CartCandidate {
    x: i32,
    y: i32,
    score: i32,
}

#[derive(Clone, Copy, Debug)]
struct MotionState {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    last_t: f64,
    lost_frames: i32,
    seen_frames: i32,
    score: f32,
}

impl MotionState {
    fn speed(self) -> f32 {
        (self.vx * self.vx + self.vy * self.vy).sqrt()
    }
}

#[derive(Clone, Debug)]
struct Args {
    fps: f64,
    log_file: String,
    template: String,
    search_top: f32,
    search_margin: f32,
    cart_detection_y: f32,
    cart_speed: f32,
    deadzone: f32,
    lost_keep_frames: i32,
    roi_padding: i32,
    roi_speed: f32,
    lost_roi_grow: i32,
    boot_size_max_scale: f32,
    rescue_space_delay: f64,
    rescue_space_taps: i32,
    rescue_space_interval: f64,
    rescue_space_cooldown: f64,
    no_rescue_space: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            fps: 60.0,
            log_file: "boot_breaker_debug.tsv".to_string(),
            template: "boot_flying_template.png".to_string(),
            search_top: 0.15,
            search_margin: 0.04,
            cart_detection_y: 0.85,
            cart_speed: 430.0,
            deadzone: 18.0,
            lost_keep_frames: 12,
            roi_padding: 28,
            roi_speed: 0.045,
            lost_roi_grow: 14,
            boot_size_max_scale: 1.45,
            rescue_space_delay: 2.0,
            rescue_space_taps: 2,
            rescue_space_interval: 0.35,
            rescue_space_cooldown: 3.0,
            no_rescue_space: false,
        }
    }
}

fn stop_requested(overlay: &OverlayHandle) -> bool {
    STOP.load(Ordering::SeqCst) || overlay.should_stop()
}

fn wait_or_stop(overlay: &OverlayHandle, duration: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < duration {
        if stop_requested(overlay) {
            return true;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        sleep(remaining.min(Duration::from_millis(20)));
    }
    stop_requested(overlay)
}

fn press_key_for(overlay: &OverlayHandle, vk: u16, duration: Duration) -> bool {
    key_event(vk, false);
    if wait_or_stop(overlay, duration) {
        key_event(vk, true);
        return false;
    }
    key_event(vk, true);
    true
}

fn tap_space_for(overlay: &OverlayHandle, duration: Duration) -> bool {
    key_event(VK_SPACE, false);
    if wait_or_stop(overlay, duration) {
        key_event(VK_SPACE, true);
        return false;
    }
    key_event(VK_SPACE, true);
    true
}

fn run_boot_throw_startup(overlay: &OverlayHandle) -> bool {
    overlay.set_lines(["status: throwing first boot"]);
    if !press_key_for(overlay, VK_D, Duration::from_millis(600)) {
        return false;
    }
    for _ in 0..2 {
        if !tap_space_for(overlay, Duration::from_millis(250)) {
            return false;
        }
        if wait_or_stop(overlay, Duration::from_millis(180)) {
            return false;
        }
    }
    if !press_key_for(overlay, VK_A, Duration::from_millis(600)) {
        return false;
    }
    for _ in 0..2 {
        if !tap_space_for(overlay, Duration::from_millis(250)) {
            return false;
        }
        if wait_or_stop(overlay, Duration::from_millis(180)) {
            return false;
        }
    }
    true
}

fn read_png_size(path: &str) -> io::Result<BootSize> {
    let mut f = File::open(path)?;
    let mut header = [0u8; 24];
    f.read_exact(&mut header)?;
    let png_sig = [137, 80, 78, 71, 13, 10, 26, 10];
    if header[0..8] != png_sig || &header[12..16] != b"IHDR" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "template must be PNG",
        ));
    }
    let w = i32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let h = i32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    Ok(BootSize { w, h })
}

fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    v.max(lo).min(hi)
}

fn base_search_roi(width: i32, height: i32, args: &Args) -> Roi {
    let x0 = (width as f32 * args.search_margin) as i32;
    let x1 = (width as f32 * (1.0 - args.search_margin)) as i32;
    let y0 = (height as f32 * args.search_top) as i32;
    let y1 = (height as f32 * args.cart_detection_y) as i32;
    Roi {
        x: x0,
        y: y0,
        w: (x1 - x0).max(1),
        h: (y1 - y0).max(1),
    }
}

fn intersect(a: Roi, b: Roi) -> Option<Roi> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    if x1 <= x0 + 8 || y1 <= y0 + 8 {
        None
    } else {
        Some(Roi {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }
}

fn roi_around(x: f32, y: f32, radius: i32, width: i32, height: i32, base: Roi) -> Roi {
    let local = Roi {
        x: (x - radius as f32).round() as i32,
        y: (y - radius as f32).round() as i32,
        w: radius * 2,
        h: radius * 2,
    };
    let screen = Roi {
        x: 0,
        y: 0,
        w: width,
        h: height,
    };
    intersect(local, screen)
        .and_then(|r| intersect(r, base))
        .unwrap_or(base)
}

fn idx(width: i32, x: i32, y: i32) -> usize {
    ((y * width + x) * 4) as usize
}

fn bgra_to_gray(frame: &[u8], width: i32, height: i32, out: &mut Vec<u8>) {
    out.resize((width * height) as usize, 0);
    for y in 0..height {
        for x in 0..width {
            let i = idx(width, x, y);
            let b = frame[i] as u32;
            let g = frame[i + 1] as u32;
            let r = frame[i + 2] as u32;
            out[(y * width + x) as usize] = ((77 * r + 150 * g + 29 * b) >> 8) as u8;
        }
    }
}

fn hsv_like(r: u8, g: u8, b: u8) -> (i32, i32, i32) {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max == 0 { 0 } else { d * 255 / max };
    let mut h = if d == 0 {
        0
    } else if max == r {
        30 * (g - b) / d
    } else if max == g {
        60 + 30 * (b - r) / d
    } else {
        120 + 30 * (r - g) / d
    };
    if h < 0 {
        h += 180;
    }
    (h, s, max)
}

fn boot_color(frame: &[u8], width: i32, x: i32, y: i32) -> bool {
    let i = idx(width, x, y);
    let (h, s, v) = hsv_like(frame[i + 2], frame[i + 1], frame[i]);
    (3..=36).contains(&h) && s >= 35 && v >= 30 && v <= 245
}

fn red_color(frame: &[u8], width: i32, x: i32, y: i32) -> bool {
    let i = idx(width, x, y);
    let (h, s, v) = hsv_like(frame[i + 2], frame[i + 1], frame[i]);
    ((0..=20).contains(&h) || (150..=179).contains(&h)) && s >= 45 && v >= 35
}

fn find_cart_candidates(
    frame: &[u8],
    width: i32,
    height: i32,
    cart_detection_y: f32,
) -> Vec<CartCandidate> {
    let half_band = 0.06;
    let y0 = (height as f32 * (cart_detection_y - half_band).max(0.0)) as i32;
    let y1 = (height as f32 * (cart_detection_y + half_band).min(1.0)) as i32;
    let margin = (width as f32 * 0.08) as i32;
    let mut col_counts = vec![0i32; width as usize];
    for y in y0..y1 {
        for x in margin..(width - margin) {
            if red_color(frame, width, x, y) {
                col_counts[x as usize] += 1;
            }
        }
    }
    let mut candidates = Vec::new();
    let mut start: Option<i32> = None;
    for x in 0..=width {
        let active = x < width && col_counts[x as usize] >= 3;
        match (active, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                let len = x - s;
                if len >= (width as f32 * 0.08) as i32 && len <= (width as f32 * 0.34) as i32 {
                    let sum: i32 = col_counts[s as usize..x as usize].iter().sum();
                    candidates.push(CartCandidate {
                        x: (s + x) / 2,
                        y: (height as f32 * cart_detection_y) as i32,
                        score: sum,
                    });
                }
                start = None;
            }
            _ => {}
        }
    }
    candidates
}

fn choose_cart_candidate(
    candidates: &[CartCandidate],
    state: Option<MotionState>,
    key: HeldKey,
    now: f64,
    width: i32,
    args: &Args,
) -> Option<(i32, i32)> {
    if candidates.is_empty() {
        return None;
    }

    let Some(s) = state else {
        return candidates
            .iter()
            .max_by_key(|candidate| candidate.score)
            .map(|candidate| (candidate.x, candidate.y));
    };

    let dt = (now - s.last_t).max(0.0) as f32;
    let predicted_vx = match key {
        HeldKey::A => -args.cart_speed.abs(),
        HeldKey::D => args.cart_speed.abs(),
        HeldKey::None => s.vx * 0.82,
    };
    let predicted_x = clamp_f32(s.x + predicted_vx * dt, 0.0, width as f32);
    let max_jump =
        (args.cart_speed.abs() * dt * 1.8 + 70.0 + s.lost_frames as f32 * 20.0).clamp(90.0, 220.0);

    candidates
        .iter()
        .filter_map(|candidate| {
            let dx = (candidate.x as f32 - predicted_x).abs();
            if dx > max_jump {
                return None;
            }
            Some((candidate, dx - candidate.score as f32 * 0.003))
        })
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(candidate, _)| (candidate.x, candidate.y))
}

fn detect_boot_motion(
    frame: &[u8],
    gray: &[u8],
    prev_gray: Option<&[u8]>,
    width: i32,
    roi: Roi,
    boot_size: BootSize,
    max_scale: f32,
    source: &'static str,
) -> Option<Detection> {
    let prev = prev_gray?;
    let max_w = (boot_size.w as f32 * max_scale) as i32;
    let max_h = (boot_size.h as f32 * max_scale) as i32;
    let mut visited = vec![false; (roi.w * roi.h) as usize];
    let mut best: Option<Detection> = None;
    let dirs = [(1, 0), (-1, 0), (0, 1), (0, -1)];

    for yy in 0..roi.h {
        for xx in 0..roi.w {
            let local_i = (yy * roi.w + xx) as usize;
            if visited[local_i] {
                continue;
            }
            let x = roi.x + xx;
            let y = roi.y + yy;
            let gi = (y * width + x) as usize;
            let motion = (gray[gi] as i32 - prev[gi] as i32).abs() >= 18;
            if !motion || !boot_color(frame, width, x, y) {
                visited[local_i] = true;
                continue;
            }

            let mut stack = vec![(xx, yy)];
            visited[local_i] = true;
            let mut min_x = xx;
            let mut max_x = xx;
            let mut min_y = yy;
            let mut max_y = yy;
            let mut count = 0i32;

            while let Some((cx, cy)) = stack.pop() {
                count += 1;
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);
                for (dx, dy) in dirs {
                    let nx = cx + dx;
                    let ny = cy + dy;
                    if nx < 0 || ny < 0 || nx >= roi.w || ny >= roi.h {
                        continue;
                    }
                    let ni = (ny * roi.w + nx) as usize;
                    if visited[ni] {
                        continue;
                    }
                    let px = roi.x + nx;
                    let py = roi.y + ny;
                    let pgi = (py * width + px) as usize;
                    let is_motion = (gray[pgi] as i32 - prev[pgi] as i32).abs() >= 18;
                    if is_motion && boot_color(frame, width, px, py) {
                        visited[ni] = true;
                        stack.push((nx, ny));
                    } else {
                        visited[ni] = true;
                    }
                }
            }

            let w = max_x - min_x + 1;
            let h = max_y - min_y + 1;
            if count < 35 || w < 8 || h < 8 || w > max_w || h > max_h {
                continue;
            }
            let aspect = w as f32 / h.max(1) as f32;
            if !(0.25..=3.2).contains(&aspect) {
                continue;
            }
            let fill = count as f32 / (w * h).max(1) as f32;
            let size_score = (count as f32 / 260.0).min(1.0);
            let shape_score = 1.0 - ((aspect - 0.85).abs() / 2.2).min(1.0);
            let cx_abs = roi.x + (min_x + max_x) / 2;
            let cy_abs = roi.y + (min_y + max_y) / 2;
            let score = 0.40 * fill + 0.45 * size_score + 0.15 * shape_score;
            if best.map_or(true, |b| score > b.score) {
                best = Some(Detection {
                    x: cx_abs,
                    y: cy_abs,
                    score,
                    bbox: Roi {
                        x: roi.x + min_x,
                        y: roi.y + min_y,
                        w,
                        h,
                    },
                    source,
                });
            }
        }
    }
    best
}

fn boot_search_rois(
    state: MotionState,
    width: i32,
    height: i32,
    base: Roi,
    boot_size: BootSize,
    args: &Args,
) -> Vec<Roi> {
    if state.lost_frames >= 7 {
        return vec![base];
    }

    let boot_diag = ((boot_size.w * boot_size.w + boot_size.h * boot_size.h) as f32).sqrt();
    let base_radius = (boot_diag * 0.5).ceil() as i32 + args.roi_padding;
    let speed_extra = (state.speed() * args.roi_speed) as i32;
    let lost_extra = state.lost_frames * args.lost_roi_grow;
    let radius = (base_radius + speed_extra + lost_extra).clamp(90, 280);

    vec![roi_around(state.x, state.y, radius, width, height, base)]
}

fn boot_settled_on_cart(boot: MotionState, cart: MotionState) -> bool {
    if boot.lost_frames > 2 || boot.seen_frames < 2 {
        return false;
    }

    let dx = (boot.x - cart.x).abs();
    let y_gap = cart.y - boot.y;
    let relative_vx = (boot.vx - cart.vx).abs();

    dx <= 48.0 && (28.0..=88.0).contains(&y_gap) && boot.vy.abs() <= 180.0 && relative_vx <= 260.0
}

fn update_boot(
    state: &mut Option<MotionState>,
    prev_det: &mut Option<(f64, f32, f32)>,
    det: Detection,
    now: f64,
) {
    let (vx, vy, seen) = if let Some((pt, px, py)) = *prev_det {
        let dt = (now - pt).max(1e-4) as f32;
        let raw_vx = (det.x as f32 - px) / dt;
        let raw_vy = (det.y as f32 - py) / dt;
        if let Some(s) = *state {
            let alpha = 0.45;
            (
                s.vx * (1.0 - alpha) + raw_vx * alpha,
                s.vy * (1.0 - alpha) + raw_vy * alpha,
                s.seen_frames + 1,
            )
        } else {
            (raw_vx, raw_vy, 1)
        }
    } else {
        (0.0, 0.0, 1)
    };
    *state = Some(MotionState {
        x: det.x as f32,
        y: det.y as f32,
        vx,
        vy,
        last_t: now,
        lost_frames: 0,
        seen_frames: seen,
        score: det.score,
    });
    *prev_det = Some((now, det.x as f32, det.y as f32));
}

fn mark_lost(state: &mut Option<MotionState>, now: f64, _width: i32, _height: i32, _args: &Args) {
    if let Some(s) = *state {
        let mut lost = s;
        lost.last_t = now;
        lost.lost_frames = s.lost_frames + 1;
        lost.score *= 0.92;
        *state = Some(lost);
    }
}

fn accept_boot_detection(state: Option<MotionState>, det: Detection, now: f64) -> bool {
    let Some(s) = state else {
        return true;
    };
    if s.seen_frames < 2 || s.lost_frames > 6 {
        return true;
    }
    let dt = (now - s.last_t).max(0.0) as f32;
    let dx = det.x as f32 - s.x;
    let dy = det.y as f32 - s.y;
    let dist = (dx * dx + dy * dy).sqrt();
    let gate = 180.0 + s.speed() * dt * 0.35 + s.lost_frames as f32 * 55.0;
    dist <= gate || det.score >= 0.82
}

fn update_cart(
    state: &mut Option<MotionState>,
    det: Option<(i32, i32)>,
    now: f64,
    width: i32,
    height: i32,
    _key: HeldKey,
    args: &Args,
) -> MotionState {
    let fallback_y = height as f32 * args.cart_detection_y;
    match (det, *state) {
        (Some((x, y)), Some(s)) => {
            let dt = (now - s.last_t).max(1e-4) as f32;
            let raw_vx = (x as f32 - s.x) / dt;
            let vx = s.vx * 0.5 + raw_vx * 0.5;
            let next = MotionState {
                x: x as f32,
                y: y as f32,
                vx,
                vy: 0.0,
                last_t: now,
                lost_frames: 0,
                seen_frames: s.seen_frames + 1,
                score: 1.0,
            };
            *state = Some(next);
            next
        }
        (Some((x, y)), None) => {
            let next = MotionState {
                x: x as f32,
                y: y as f32,
                vx: 0.0,
                vy: 0.0,
                last_t: now,
                lost_frames: 0,
                seen_frames: 1,
                score: 1.0,
            };
            *state = Some(next);
            next
        }
        (None, Some(s)) => {
            let next = MotionState {
                x: s.x,
                y: s.y,
                vx: s.vx * 0.75,
                vy: 0.0,
                last_t: now,
                lost_frames: s.lost_frames + 1,
                seen_frames: s.seen_frames,
                score: s.score * 0.95,
            };
            *state = Some(next);
            next
        }
        (None, None) => {
            let next = MotionState {
                x: width as f32 / 2.0,
                y: fallback_y,
                vx: 0.0,
                vy: 0.0,
                last_t: now,
                lost_frames: 1,
                seen_frames: 0,
                score: 0.0,
            };
            *state = Some(next);
            next
        }
    }
}

fn steer_to(
    target_x: f32,
    cart: MotionState,
    intercept_t: f32,
    held: &mut HeldKey,
    args: &Args,
) -> (char, f32) {
    let lookahead = (intercept_t * 0.35).clamp(0.0, 0.18);
    let error = target_x - (cart.x + cart.vx * lookahead);
    let switch_zone = args.deadzone * 1.45;
    let release_zone = args.deadzone * 0.55;
    match *held {
        HeldKey::D => {
            if error < -switch_zone {
                hold_key(held, HeldKey::A);
                ('A', error)
            } else if error > release_zone {
                ('D', error)
            } else {
                release_keys(held, false);
                ('-', error)
            }
        }
        HeldKey::A => {
            if error > switch_zone {
                hold_key(held, HeldKey::D);
                ('D', error)
            } else if error < -release_zone {
                ('A', error)
            } else {
                release_keys(held, false);
                ('-', error)
            }
        }
        HeldKey::None => {
            if error > args.deadzone {
                hold_key(held, HeldKey::D);
                ('D', error)
            } else if error < -args.deadzone {
                hold_key(held, HeldKey::A);
                ('A', error)
            } else {
                ('-', error)
            }
        }
    }
}

fn open_log(path: &str) -> io::Result<Option<BufWriter<File>>> {
    if path.is_empty() {
        return Ok(None);
    }
    let mut w = BufWriter::new(File::create(path)?);
    writeln!(
        w,
        "frame\ttime\tboot_det_x\tboot_det_y\tboot_score\tboot_source\tboot_box\tboot_track_x\tboot_track_y\tboot_vx\tboot_vy\tboot_lost\tcart_det_x\tcart_det_y\tcart_track_x\tcart_track_y\tcart_vx\tcart_lost\tcart_candidate_count\tcart_candidates\tcart_choice_reason\ttarget_x\ttarget_source\terror\taction\tkey\tcontrol_reason\treject_reason\tboot_search_mode\tloop_ms\tfps\tboot_expected_w\tboot_expected_h\tsearch_roi_count\tsearch_roi_area\tsearch_rois\tgrab_ms\tgray_ms\tcart_ms\tboot_search_ms\tintercept_ms\tcontrol_ms\tlog_ms"
    )?;
    Ok(Some(w))
}

#[cfg(debug_assertions)]
fn live_debug_primitives(
    screen_rect: win32::Rect,
    width: i32,
    height: i32,
    base_roi: Roi,
    search_rois: &[Roi],
    boot_det: Option<Detection>,
    boot_state: Option<MotionState>,
    cart_det: Option<(i32, i32)>,
    cart: MotionState,
    target_x: Option<f32>,
    action: char,
    reject_reason: &str,
    args: &Args,
) -> Vec<OverlayPrimitive> {
    let mut items = Vec::new();
    push_rect(
        &mut items,
        screen_rect,
        Roi {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        rgb(255, 255, 255),
    );
    push_rect(&mut items, screen_rect, base_roi, rgb(120, 120, 120));
    for roi in search_rois {
        push_rect(&mut items, screen_rect, *roi, rgb(0, 180, 255));
    }
    let search_bottom_y = (height as f32 * args.cart_detection_y) as i32;
    push_line(
        &mut items,
        screen_rect,
        0,
        search_bottom_y,
        width - 1,
        search_bottom_y,
        rgb(255, 0, 0),
    );
    let cart_line_y = (height as f32 * args.cart_detection_y) as i32;
    push_line(
        &mut items,
        screen_rect,
        0,
        cart_line_y,
        width - 1,
        cart_line_y,
        rgb(255, 0, 255),
    );
    if let Some(det) = boot_det {
        push_rect(&mut items, screen_rect, det.bbox, rgb(0, 255, 0));
        push_cross(&mut items, screen_rect, det.x, det.y, 16, rgb(0, 255, 0));
    }
    if let Some(state) = boot_state {
        let x = state.x.round() as i32;
        let y = state.y.round() as i32;
        push_cross(&mut items, screen_rect, x, y, 22, rgb(255, 255, 0));
        push_line(
            &mut items,
            screen_rect,
            x,
            y,
            (state.x + state.vx * 0.12).round() as i32,
            (state.y + state.vy * 0.12).round() as i32,
            rgb(255, 255, 0),
        );
    }
    if let Some((x, y)) = cart_det {
        push_cross(&mut items, screen_rect, x, y, 18, rgb(255, 0, 255));
    }
    let cart_x = cart.x.round() as i32;
    let cart_y = cart.y.round() as i32;
    push_cross(&mut items, screen_rect, cart_x, cart_y, 24, rgb(0, 0, 255));
    push_line(
        &mut items,
        screen_rect,
        cart_x,
        cart_y,
        (cart.x + cart.vx * 0.12).round() as i32,
        cart_y,
        rgb(0, 0, 255),
    );
    if let Some(target) = target_x {
        let x = target.round() as i32;
        push_line(
            &mut items,
            screen_rect,
            x,
            0,
            x,
            height - 1,
            rgb(0, 255, 255),
        );
    }
    let action_color = match action {
        'A' => rgb(255, 80, 80),
        'D' => rgb(80, 80, 255),
        'S' => rgb(80, 255, 80),
        _ => rgb(180, 180, 180),
    };
    push_rect(
        &mut items,
        screen_rect,
        Roi {
            x: 8,
            y: 8,
            w: 54,
            h: 34,
        },
        action_color,
    );
    if !reject_reason.is_empty() {
        push_rect(
            &mut items,
            screen_rect,
            Roi {
                x: 4,
                y: 4,
                w: width - 8,
                h: height - 8,
            },
            rgb(255, 0, 0),
        );
    }
    items
}

#[cfg(debug_assertions)]
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

#[cfg(debug_assertions)]
fn push_rect(items: &mut Vec<OverlayPrimitive>, screen_rect: win32::Rect, rect: Roi, color: u32) {
    items.push(OverlayPrimitive::Rect {
        x: screen_rect.left + rect.x,
        y: screen_rect.top + rect.y,
        w: rect.w,
        h: rect.h,
        color,
    });
}

#[cfg(debug_assertions)]
fn push_line(
    items: &mut Vec<OverlayPrimitive>,
    screen_rect: win32::Rect,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    items.push(OverlayPrimitive::Line {
        x0: screen_rect.left + x0,
        y0: screen_rect.top + y0,
        x1: screen_rect.left + x1,
        y1: screen_rect.top + y1,
        color,
    });
}

#[cfg(debug_assertions)]
fn push_cross(
    items: &mut Vec<OverlayPrimitive>,
    screen_rect: win32::Rect,
    x: i32,
    y: i32,
    radius: i32,
    color: u32,
) {
    items.push(OverlayPrimitive::Cross {
        x: screen_rect.left + x,
        y: screen_rect.top + y,
        radius,
        color,
    });
}

#[cfg(debug_assertions)]
fn paint_debug_frame(
    frame: &[u8],
    width: i32,
    height: i32,
    frame_index: u64,
    base_roi: Roi,
    search_rois: &[Roi],
    boot_det: Option<Detection>,
    boot_state: Option<MotionState>,
    cart_det: Option<(i32, i32)>,
    cart: MotionState,
    target_x: Option<f32>,
    action: char,
    reject_reason: &str,
    args: &Args,
) -> io::Result<()> {
    std::fs::create_dir_all("boot_breaker_debug_frames")?;
    let mut image = frame.to_vec();
    draw_rect(
        &mut image,
        width,
        height,
        Roi {
            x: 0,
            y: 0,
            w: width,
            h: height,
        },
        [255, 255, 255],
    );
    draw_rect(&mut image, width, height, base_roi, [120, 120, 120]);
    for roi in search_rois {
        draw_rect(&mut image, width, height, *roi, [0, 180, 255]);
    }
    let search_bottom_y = (height as f32 * args.cart_detection_y) as i32;
    draw_hline(
        &mut image,
        width,
        height,
        search_bottom_y,
        0,
        width - 1,
        [0, 0, 255],
    );
    let cart_line_y = (height as f32 * args.cart_detection_y) as i32;
    draw_hline(
        &mut image,
        width,
        height,
        cart_line_y,
        0,
        width - 1,
        [255, 0, 255],
    );
    if let Some(det) = boot_det {
        draw_rect(&mut image, width, height, det.bbox, [0, 255, 0]);
        draw_cross(&mut image, width, height, det.x, det.y, 12, [0, 255, 0]);
    }
    if let Some(state) = boot_state {
        let x = state.x.round() as i32;
        let y = state.y.round() as i32;
        draw_cross(&mut image, width, height, x, y, 18, [255, 255, 0]);
        draw_vector(
            &mut image,
            width,
            height,
            x,
            y,
            (state.x + state.vx * 0.12).round() as i32,
            (state.y + state.vy * 0.12).round() as i32,
            [255, 255, 0],
        );
    }
    if let Some((x, y)) = cart_det {
        draw_cross(&mut image, width, height, x, y, 16, [255, 0, 255]);
    }
    let cart_x = cart.x.round() as i32;
    let cart_y = cart.y.round() as i32;
    draw_cross(&mut image, width, height, cart_x, cart_y, 20, [255, 0, 0]);
    draw_vector(
        &mut image,
        width,
        height,
        cart_x,
        cart_y,
        (cart.x + cart.vx * 0.12).round() as i32,
        cart_y,
        [255, 0, 0],
    );
    if let Some(target) = target_x {
        let x = target.round() as i32;
        draw_vline(&mut image, width, height, x, 0, height - 1, [0, 255, 255]);
    }
    let action_color = match action {
        'A' => [255, 80, 80],
        'D' => [80, 80, 255],
        'S' => [80, 255, 80],
        _ => [180, 180, 180],
    };
    draw_rect(
        &mut image,
        width,
        height,
        Roi {
            x: 8,
            y: 8,
            w: 54,
            h: 34,
        },
        action_color,
    );
    if !reject_reason.is_empty() {
        draw_rect(
            &mut image,
            width,
            height,
            Roi {
                x: 4,
                y: 4,
                w: width - 8,
                h: height - 8,
            },
            [0, 0, 255],
        );
    }
    write_bmp(
        "boot_breaker_debug_frames/latest.bmp",
        &image,
        width,
        height,
    )?;
    if frame_index % 30 == 0 {
        write_bmp(
            &format!("boot_breaker_debug_frames/frame_{frame_index:06}.bmp"),
            &image,
            width,
            height,
        )?;
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn draw_rect(image: &mut [u8], width: i32, height: i32, rect: Roi, color: [u8; 3]) {
    let x0 = rect.x.clamp(0, width - 1);
    let y0 = rect.y.clamp(0, height - 1);
    let x1 = (rect.x + rect.w - 1).clamp(0, width - 1);
    let y1 = (rect.y + rect.h - 1).clamp(0, height - 1);
    draw_hline(image, width, height, y0, x0, x1, color);
    draw_hline(image, width, height, y1, x0, x1, color);
    draw_vline(image, width, height, x0, y0, y1, color);
    draw_vline(image, width, height, x1, y0, y1, color);
}

#[cfg(debug_assertions)]
fn draw_cross(
    image: &mut [u8],
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    radius: i32,
    color: [u8; 3],
) {
    draw_hline(image, width, height, y, x - radius, x + radius, color);
    draw_vline(image, width, height, x, y - radius, y + radius, color);
}

#[cfg(debug_assertions)]
fn draw_hline(image: &mut [u8], width: i32, height: i32, y: i32, x0: i32, x1: i32, color: [u8; 3]) {
    if y < 0 || y >= height {
        return;
    }
    let start = x0.min(x1).clamp(0, width - 1);
    let end = x0.max(x1).clamp(0, width - 1);
    for x in start..=end {
        set_pixel(image, width, height, x, y, color);
    }
}

#[cfg(debug_assertions)]
fn draw_vline(image: &mut [u8], width: i32, height: i32, x: i32, y0: i32, y1: i32, color: [u8; 3]) {
    if x < 0 || x >= width {
        return;
    }
    let start = y0.min(y1).clamp(0, height - 1);
    let end = y0.max(y1).clamp(0, height - 1);
    for y in start..=end {
        set_pixel(image, width, height, x, y, color);
    }
}

#[cfg(debug_assertions)]
fn draw_vector(
    image: &mut [u8],
    width: i32,
    height: i32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: [u8; 3],
) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        set_pixel(image, width, height, x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = err * 2;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

#[cfg(debug_assertions)]
fn set_pixel(image: &mut [u8], width: i32, height: i32, x: i32, y: i32, color: [u8; 3]) {
    if x < 0 || y < 0 || x >= width || y >= height {
        return;
    }
    let i = ((y * width + x) * 4) as usize;
    image[i] = color[2];
    image[i + 1] = color[1];
    image[i + 2] = color[0];
    image[i + 3] = 255;
}

#[cfg(debug_assertions)]
fn write_bmp(path: &str, image: &[u8], width: i32, height: i32) -> io::Result<()> {
    let mut file = File::create(path)?;
    let pixel_bytes = (width * height * 4) as u32;
    let file_size = 14u32 + 40u32 + pixel_bytes;
    file.write_all(b"BM")?;
    file.write_all(&file_size.to_le_bytes())?;
    file.write_all(&[0, 0, 0, 0])?;
    file.write_all(&(54u32).to_le_bytes())?;
    file.write_all(&(40u32).to_le_bytes())?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&(-height).to_le_bytes())?;
    file.write_all(&(1u16).to_le_bytes())?;
    file.write_all(&(32u16).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(&pixel_bytes.to_le_bytes())?;
    file.write_all(&(0i32).to_le_bytes())?;
    file.write_all(&(0i32).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(image)
}

pub fn run() -> ToolResult {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }

    let args = Args::default();
    let overlay = overlay::start("Boot Breaker");
    overlay.set_target_fps(args.fps);
    overlay.set_lines(["status: launching minigame"]);

    unsafe {
        win32::SetConsoleCtrlHandler(Some(console_ctrl_handler), 1);
    }

    let boot_size = read_png_size(&args.template).unwrap_or(BootSize { w: 88, h: 112 });
    overlay.set_lines([
        format!("template: {}x{}", boot_size.w, boot_size.h),
        "launch: waiting for PLAY".to_string(),
        "Ctrl+Shift+Q: stop".to_string(),
    ]);
    let Some(rect) =
        launch_minigame_with_log(&overlay, LaunchProfile::BootBreaker, &args.log_file)?
    else {
        overlay.set_lines(["status: stopped before game area detection"]);
        return Ok(());
    };

    if stop_requested(&overlay) {
        return Ok(());
    }
    overlay.set_lines(["status: game area detected"]);

    let mut held = HeldKey::None;
    let mut cap = ScreenCapture::new(rect.width, rect.height)?;

    let startup_done = Arc::new(AtomicBool::new(false));
    let startup_ok = Arc::new(AtomicBool::new(false));
    let startup_overlay = overlay.clone();
    let startup_done_worker = Arc::clone(&startup_done);
    let startup_ok_worker = Arc::clone(&startup_ok);
    let startup_handle = thread::spawn(move || {
        let ok = run_boot_throw_startup(&startup_overlay);
        startup_ok_worker.store(ok, Ordering::SeqCst);
        startup_done_worker.store(true, Ordering::SeqCst);
    });
    let mut gray = Vec::new();
    let mut prev_gray: Option<Vec<u8>> = None;
    let mut boot_state: Option<MotionState> = None;
    let mut prev_boot_det: Option<(f64, f32, f32)> = None;
    let mut cart_state: Option<MotionState> = None;
    let mut log = open_log(&args.log_file)?;
    let mut boot_missing_since: Option<f64> = None;
    let mut rescue_space_remaining = 0;
    let mut next_rescue_space_at = f64::INFINITY;
    let mut boot_on_cart_since: Option<f64> = None;
    let mut next_cart_relaunch_at = 0.0f64;
    let frame_delay = Duration::from_secs_f64(1.0 / args.fps.max(1.0));
    let start = Instant::now();
    let mut fps_est = 0.0f64;
    let mut frame_index = 0u64;

    while !stop_requested(&overlay) {
        let loop_t0 = Instant::now();
        let now = start.elapsed().as_secs_f64();
        let t = Instant::now();
        let frame = cap.grab_bgra(rect)?;
        let grab_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        bgra_to_gray(frame, rect.width, rect.height, &mut gray);
        let gray_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let cart_candidates =
            find_cart_candidates(frame, rect.width, rect.height, args.cart_detection_y);
        let cart_det =
            choose_cart_candidate(&cart_candidates, cart_state, held, now, rect.width, &args);
        let cart_choice_reason = match (cart_candidates.is_empty(), cart_det) {
            (true, _) => "no_red_candidates",
            (false, Some(_)) => "selected",
            (false, None) => "rejected_by_cart_gate",
        };
        let cart = update_cart(
            &mut cart_state,
            cart_det,
            now,
            rect.width,
            rect.height,
            held,
            &args,
        );
        let cart_ms = t.elapsed().as_secs_f64() * 1000.0;

        let base_roi = base_search_roi(rect.width, rect.height, &args);
        let boot_search_state = boot_state;

        let t = Instant::now();
        let mut search_rois = Vec::new();
        let mut boot_det = None;
        let mut boot_search_mode = "full-init";
        if let Some(search_state) = boot_search_state {
            search_rois = boot_search_rois(
                search_state,
                rect.width,
                rect.height,
                base_roi,
                boot_size,
                &args,
            );
            for roi in search_rois.iter().copied() {
                let full_reacquire = search_state.lost_frames >= 7;
                boot_search_mode = if full_reacquire {
                    "full-reacquire"
                } else {
                    "last-known-roi"
                };
                boot_det = detect_boot_motion(
                    frame,
                    &gray,
                    prev_gray.as_deref(),
                    rect.width,
                    roi,
                    boot_size,
                    args.boot_size_max_scale,
                    if full_reacquire {
                        "playfield-reacquire"
                    } else {
                        "last-known-motion"
                    },
                );
                if boot_det.is_some() {
                    break;
                }
            }
        } else {
            search_rois.push(base_roi);
            boot_det = detect_boot_motion(
                frame,
                &gray,
                prev_gray.as_deref(),
                rect.width,
                base_roi,
                boot_size,
                args.boot_size_max_scale,
                "full-init-motion",
            );
        }
        let boot_search_ms = t.elapsed().as_secs_f64() * 1000.0;

        let mut reject_reason = String::new();

        if let Some(det) = boot_det {
            if det.source == "playfield-reacquire" && det.score < 0.70 {
                reject_reason = format!("weak_reacquire score={:.3}", det.score);
                boot_det = None;
            }
        }

        if let Some(det) = boot_det {
            let cart_line_y = rect.height as f32 * args.cart_detection_y;
            if det.y as f32 > cart_line_y {
                reject_reason = format!("boot_below_cart det_y={} cart_y={cart_line_y:.0}", det.y);
                boot_det = None;
            }
        }

        if let Some(det) = boot_det {
            if accept_boot_detection(boot_state, det, now) {
                update_boot(&mut boot_state, &mut prev_boot_det, det, now);
            } else {
                reject_reason = "tracker_gate".to_string();
                boot_det = None;
                mark_lost(&mut boot_state, now, rect.width, rect.height, &args);
            }
        } else {
            mark_lost(&mut boot_state, now, rect.width, rect.height, &args);
        }

        let startup_complete = startup_done.load(Ordering::SeqCst);
        let startup_succeeded = startup_ok.load(Ordering::SeqCst);
        let boot_unavailable = boot_state.map_or(true, |s| s.lost_frames > args.lost_keep_frames);
        if !startup_complete {
            boot_missing_since = None;
            rescue_space_remaining = 0;
            next_rescue_space_at = f64::INFINITY;
        } else if !boot_unavailable {
            boot_missing_since = None;
            rescue_space_remaining = 0;
            next_rescue_space_at = f64::INFINITY;
        } else if boot_missing_since.is_none() {
            boot_missing_since = Some(now);
            next_rescue_space_at = now + args.rescue_space_delay.max(0.0);
        }

        let boot_on_cart = boot_state
            .map(|boot| boot_settled_on_cart(boot, cart))
            .unwrap_or(false);
        if startup_complete && startup_succeeded && boot_on_cart {
            if boot_on_cart_since.is_none() {
                boot_on_cart_since = Some(now);
            }
        } else {
            boot_on_cart_since = None;
        }
        let cart_relaunch_ready = boot_on_cart_since
            .map(|since| now - since >= 0.18)
            .unwrap_or(false)
            && now >= next_cart_relaunch_at;

        let t = Instant::now();
        let target_x = boot_det.map(|det| det.x as f32);
        let target_source = if boot_det.is_some() {
            "boot_det"
        } else {
            "none"
        };
        let intercept_t = 0.0;
        let intercept_ms = t.elapsed().as_secs_f64() * 1000.0;

        let t = Instant::now();
        let mut action = '-';
        let mut error = None;
        let mut control_reason: String;
        if let Some(boot) = boot_state {
            if !startup_complete || !startup_succeeded {
                release_keys(&mut held, false);
                control_reason = if startup_complete {
                    "startup_failed".to_string()
                } else {
                    "startup_tracking".to_string()
                };
                if reject_reason.is_empty() {
                    reject_reason = control_reason.clone();
                }
            } else if boot.lost_frames <= args.lost_keep_frames
                && target_x.is_some()
                && cart_det.is_some()
            {
                let (a, e) = steer_to(target_x.unwrap(), cart, intercept_t, &mut held, &args);
                action = a;
                error = Some(e);
                control_reason = "steer_to_detected_boot".to_string();
            } else {
                release_keys(&mut held, false);
                control_reason = if target_x.is_none() {
                    "no_current_boot_detection".to_string()
                } else if cart_det.is_none() {
                    "no_current_cart_detection".to_string()
                } else {
                    format!("boot_lost={}", boot.lost_frames)
                };
            }
        } else {
            release_keys(&mut held, false);
            control_reason = "boot_state_none".to_string();
        }
        if cart_relaunch_ready {
            release_keys(&mut held, false);
            tap_space();
            action = 'S';
            reject_reason = "boot_on_cart_relaunch".to_string();
            control_reason = "boot_on_cart_relaunch".to_string();

            next_cart_relaunch_at = now + 0.75;
            boot_on_cart_since = None;
            boot_state = None;
            prev_boot_det = None;
            boot_missing_since = None;
            rescue_space_remaining = 0;
            next_rescue_space_at = f64::INFINITY;
        } else if startup_complete
            && startup_succeeded
            && boot_unavailable
            && !args.no_rescue_space
            && args.rescue_space_taps > 0
            && now >= next_rescue_space_at
        {
            release_keys(&mut held, false);
            tap_space();
            action = 'S';
            control_reason = "rescue_space".to_string();
            reject_reason = if reject_reason.is_empty() {
                "rescue_space".to_string()
            } else {
                reject_reason
            };
            if rescue_space_remaining <= 0 {
                rescue_space_remaining = args.rescue_space_taps - 1;
            } else {
                rescue_space_remaining -= 1;
            }
            next_rescue_space_at = if rescue_space_remaining > 0 {
                now + args.rescue_space_interval.max(0.05)
            } else {
                now + args
                    .rescue_space_cooldown
                    .max(args.rescue_space_interval.max(0.05))
            };
        }
        let control_ms = t.elapsed().as_secs_f64() * 1000.0;

        let elapsed = loop_t0.elapsed();
        let fps_now = 1.0 / elapsed.as_secs_f64().max(1e-6);
        fps_est = if fps_est <= 0.0 {
            fps_now
        } else {
            fps_est * 0.85 + fps_now * 0.15
        };

        let boot_line = boot_state.map_or_else(
            || "boot: missing".to_string(),
            |s| {
                format!(
                    "boot: x={:.0} y={:.0} vx={:.0} vy={:.0} lost={}",
                    s.x, s.y, s.vx, s.vy, s.lost_frames
                )
            },
        );
        let cart_line = format!(
            "cart: x={:.0} vx={:.0} lost={} target={}",
            cart.x,
            cart.vx,
            cart.lost_frames,
            target_x.map_or("-".to_string(), |x| format!("{x:.0}"))
        );
        let control_line = format!("control: action={action} key={held:?} {control_reason}");
        let status_line = if reject_reason.is_empty() {
            "status: running".to_string()
        } else {
            format!("status: {reject_reason}")
        };
        overlay.update(
            fps_est,
            args.fps,
            [control_line, boot_line, cart_line, status_line],
        );

        #[cfg(debug_assertions)]
        overlay.set_debug_primitives(live_debug_primitives(
            rect,
            rect.width,
            rect.height,
            base_roi,
            &search_rois,
            boot_det,
            boot_state,
            cart_det,
            cart,
            target_x,
            action,
            &reject_reason,
            &args,
        ));

        #[cfg(debug_assertions)]
        if let Err(err) = paint_debug_frame(
            frame,
            rect.width,
            rect.height,
            frame_index,
            base_roi,
            &search_rois,
            boot_det,
            boot_state,
            cart_det,
            cart,
            target_x,
            action,
            &reject_reason,
            &args,
        ) {
            if reject_reason.is_empty() {
                reject_reason = format!("debug_paint_failed: {err}");
            }
        }

        if let Some(w) = log.as_mut() {
            let t = Instant::now();
            let roi_text = search_rois
                .iter()
                .map(|r| format!("{},{},{},{}", r.x, r.y, r.w, r.h))
                .collect::<Vec<_>>()
                .join(";");
            let cart_candidate_text = cart_candidates
                .iter()
                .map(|candidate| format!("{},{},{}", candidate.x, candidate.y, candidate.score))
                .collect::<Vec<_>>()
                .join(";");
            let roi_area: i32 = search_rois.iter().map(|r| r.w * r.h).sum();
            let bd = boot_det;
            let bs = boot_state;
            let row = vec![
                frame_index.to_string(),
                format!("{now:.6}"),
                bd.map_or(String::new(), |d| d.x.to_string()),
                bd.map_or(String::new(), |d| d.y.to_string()),
                bd.map_or(String::new(), |d| format!("{:.3}", d.score)),
                bd.map_or(String::new(), |d| d.source.to_string()),
                bd.map_or(String::new(), |d| {
                    format!("{},{},{},{}", d.bbox.x, d.bbox.y, d.bbox.w, d.bbox.h)
                }),
                bs.map_or(String::new(), |s| format!("{:.1}", s.x)),
                bs.map_or(String::new(), |s| format!("{:.1}", s.y)),
                bs.map_or(String::new(), |s| format!("{:.1}", s.vx)),
                bs.map_or(String::new(), |s| format!("{:.1}", s.vy)),
                bs.map_or(String::new(), |s| s.lost_frames.to_string()),
                cart_det.map_or(String::new(), |(x, _)| x.to_string()),
                cart_det.map_or(String::new(), |(_, y)| y.to_string()),
                format!("{:.1}", cart.x),
                format!("{:.1}", cart.y),
                format!("{:.1}", cart.vx),
                cart.lost_frames.to_string(),
                cart_candidates.len().to_string(),
                cart_candidate_text,
                cart_choice_reason.to_string(),
                target_x.map_or(String::new(), |v| format!("{v:.1}")),
                target_source.to_string(),
                error.map_or(String::new(), |v| format!("{v:.1}")),
                action.to_string(),
                format!("{held:?}"),
                control_reason.clone(),
                reject_reason.clone(),
                boot_search_mode.to_string(),
                format!("{:.2}", elapsed.as_secs_f64() * 1000.0),
                format!("{fps_est:.2}"),
                boot_size.w.to_string(),
                boot_size.h.to_string(),
                search_rois.len().to_string(),
                roi_area.to_string(),
                roi_text,
                format!("{grab_ms:.2}"),
                format!("{gray_ms:.2}"),
                format!("{cart_ms:.2}"),
                format!("{boot_search_ms:.2}"),
                format!("{intercept_ms:.2}"),
                format!("{control_ms:.2}"),
                format!("{:.2}", t.elapsed().as_secs_f64() * 1000.0),
            ];
            writeln!(w, "{}", row.join("\t"))?;
            if frame_index % 30 == 0 {
                w.flush()?;
            }
        }

        prev_gray = Some(gray.clone());
        frame_index += 1;
        if elapsed < frame_delay {
            sleep(frame_delay - elapsed);
        }
    }
    overlay.request_stop();
    let _ = startup_handle.join();
    release_keys(&mut held, true);
    key_event(VK_SPACE, true);
    if let Some(w) = log.as_mut() {
        w.flush()?;
    }
    Ok(())
}
pub struct BootBreaker;

impl Tool for BootBreaker {
    fn name(&self) -> &'static str {
        "boot_breaker"
    }

    fn run(&mut self) -> ToolResult {
        run()
    }
}

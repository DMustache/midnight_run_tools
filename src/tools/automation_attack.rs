#[cfg(target_os = "windows")]
#[allow(clashing_extern_declarations)]
mod windows_app {
    use std::collections::BTreeSet;
    use std::ffi::c_void;
    use std::fs;
    use std::io::{self, Write};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::ptr::null_mut;
    use std::sync::mpsc::{TrySendError, sync_channel};
    use std::thread::{self, sleep};
    use std::time::{Duration, Instant};

    use crate::overlay;
    use crate::platform::win32;
    use crate::tools::launcher::{LaunchProfile, launch_minigame_with_log};

    type Bool = i32;
    type Dword = u32;
    type Long = i32;
    type Uint = u32;
    type Word = u16;
    type Hwnd = *mut c_void;
    type Hdc = *mut c_void;
    type Hbitmap = *mut c_void;
    type Hgdiobj = *mut c_void;
    type UlongPtr = usize;

    const SRCCOPY: Dword = 0x00CC_0020;
    const DIB_RGB_COLORS: Uint = 0;
    const BI_RGB: Dword = 0;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const INPUT_KEYBOARD: Dword = 1;
    const KEYEVENTF_KEYUP: Dword = 0x0002;
    const KEYEVENTF_SCANCODE: Dword = 0x0008;
    const MAPVK_VK_TO_VSC: Uint = 0;
    const TEMPLATE_W: usize = 32;
    const TEMPLATE_H: usize = 40;
    const TEMPLATE_LEN: usize = TEMPLATE_W * TEMPLATE_H;
    const TEMPLATE_INNER_W: usize = 28;
    const TEMPLATE_INNER_H: usize = 36;
    const SEED_TEMPLATES: &[u8] = include_bytes!("automation_attack_seed_templates.bin");

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: Dword,
        bi_width: Long,
        bi_height: Long,
        bi_planes: Word,
        bi_bit_count: Word,
        bi_compression: Dword,
        bi_size_image: Dword,
        bi_x_pels_per_meter: Long,
        bi_y_pels_per_meter: Long,
        bi_clr_used: Dword,
        bi_clr_important: Dword,
    }

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [Dword; 1],
    }

    #[repr(C)]
    struct Input {
        r#type: Dword,
        _padding: Dword,
        ki: KeyboardInput,
        _union_padding: UlongPtr,
    }

    #[repr(C)]
    struct KeyboardInput {
        w_vk: Word,
        w_scan: Word,
        dw_flags: Dword,
        time: Dword,
        dw_extra_info: UlongPtr,
    }

    #[derive(Clone, Copy, Debug)]
    struct CaptureRect {
        left: i32,
        top: i32,
        width: i32,
        height: i32,
    }

    #[derive(Clone, Debug)]
    struct Args {
        fps: f64,
        log_file: String,
        rect: Option<CaptureRect>,
        debug: bool,
        type_words: bool,
        key_delay_ms: u64,
    }

    impl Default for Args {
        fn default() -> Self {
            Self {
                fps: 30.0,
                log_file: "automation_attack_debug.tsv".to_string(),
                rect: None,
                debug: false,
                type_words: true,
                key_delay_ms: 2,
            }
        }
    }

    #[allow(non_snake_case)]
    unsafe fn GetDC(h_wnd: Hwnd) -> Hdc {
        unsafe { win32::GetDC(h_wnd) }
    }

    #[allow(non_snake_case)]
    unsafe fn ReleaseDC(h_wnd: Hwnd, h_dc: Hdc) -> i32 {
        unsafe { win32::ReleaseDC(h_wnd, h_dc) }
    }

    #[allow(non_snake_case)]
    unsafe fn SendInput(c_inputs: Uint, p_inputs: *const Input, cb_size: i32) -> Uint {
        unsafe { win32::SendInput(c_inputs, p_inputs as *const win32::Input, cb_size) }
    }

    #[allow(non_snake_case)]
    unsafe fn MapVirtualKeyW(code: Uint, map_type: Uint) -> Uint {
        unsafe { win32::MapVirtualKeyW(code, map_type) }
    }

    #[allow(non_snake_case)]
    unsafe fn CreateCompatibleDC(hdc: Hdc) -> Hdc {
        unsafe { win32::CreateCompatibleDC(hdc) }
    }

    #[allow(non_snake_case)]
    unsafe fn CreateDIBSection(
        hdc: Hdc,
        pbmi: *const BitmapInfo,
        usage: Uint,
        ppv_bits: *mut *mut c_void,
        section: *mut c_void,
        offset: Dword,
    ) -> Hbitmap {
        unsafe {
            win32::CreateDIBSection(
                hdc,
                pbmi as *const win32::BitmapInfo,
                usage,
                ppv_bits,
                section,
                offset,
            )
        }
    }

    #[allow(non_snake_case)]
    unsafe fn SelectObject(hdc: Hdc, h: Hgdiobj) -> Hgdiobj {
        unsafe { win32::SelectObject(hdc, h) }
    }

    #[allow(non_snake_case)]
    unsafe fn BitBlt(
        hdc: Hdc,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        hdc_src: Hdc,
        x1: i32,
        y1: i32,
        rop: Dword,
    ) -> Bool {
        unsafe { win32::BitBlt(hdc, x, y, cx, cy, hdc_src, x1, y1, rop) }
    }

    #[allow(non_snake_case)]
    unsafe fn DeleteObject(ho: Hgdiobj) -> Bool {
        unsafe { win32::DeleteObject(ho) }
    }

    #[allow(non_snake_case)]
    unsafe fn DeleteDC(hdc: Hdc) -> Bool {
        unsafe { win32::DeleteDC(hdc) }
    }
    struct ScreenCapture {
        screen_dc: Hdc,
        mem_dc: Hdc,
        bitmap: Hbitmap,
        old_obj: Hgdiobj,
        width: i32,
        height: i32,
        bits: *mut u8,
        len: usize,
    }

    impl ScreenCapture {
        fn new(width: i32, height: i32) -> io::Result<Self> {
            if width <= 0 || height <= 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid capture size",
                ));
            }

            unsafe {
                let screen_dc = GetDC(null_mut());
                if screen_dc.is_null() {
                    return Err(io::Error::last_os_error());
                }

                let mem_dc = CreateCompatibleDC(screen_dc);
                if mem_dc.is_null() {
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(io::Error::last_os_error());
                }

                let info = make_bitmap_info(width, height);
                let mut bits: *mut c_void = null_mut();
                let bitmap =
                    CreateDIBSection(screen_dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);

                if bitmap.is_null() || bits.is_null() {
                    DeleteDC(mem_dc);
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(io::Error::last_os_error());
                }

                let old_obj = SelectObject(mem_dc, bitmap as Hgdiobj);
                if old_obj.is_null() {
                    DeleteObject(bitmap as Hgdiobj);
                    DeleteDC(mem_dc);
                    ReleaseDC(null_mut(), screen_dc);
                    return Err(io::Error::last_os_error());
                }

                Ok(Self {
                    screen_dc,
                    mem_dc,
                    bitmap,
                    old_obj,
                    width,
                    height,
                    bits: bits as *mut u8,
                    len: width as usize * height as usize * 4,
                })
            }
        }

        fn grab_bgra(&mut self, rect: CaptureRect) -> io::Result<&[u8]> {
            unsafe {
                if BitBlt(
                    self.mem_dc,
                    0,
                    0,
                    self.width,
                    self.height,
                    self.screen_dc,
                    rect.left,
                    rect.top,
                    SRCCOPY,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }

                Ok(std::slice::from_raw_parts(self.bits, self.len))
            }
        }
    }

    impl Drop for ScreenCapture {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.mem_dc, self.old_obj);
                DeleteObject(self.bitmap as Hgdiobj);
                DeleteDC(self.mem_dc);
                ReleaseDC(null_mut(), self.screen_dc);
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct Component {
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        area: usize,
    }

    impl Component {
        fn width(self) -> usize {
            self.x1 - self.x0 + 1
        }
        fn height(self) -> usize {
            self.y1 - self.y0 + 1
        }
        fn cx(self) -> f32 {
            (self.x0 + self.x1) as f32 * 0.5
        }
        fn cy(self) -> f32 {
            (self.y0 + self.y1) as f32 * 0.5
        }
    }

    struct ColorAnalyzer {
        width: usize,
        height: usize,
        combined: Vec<u8>,
        orange: Vec<u8>,
        white_count: usize,
        orange_count: usize,
    }

    impl ColorAnalyzer {
        fn new(width: usize, height: usize) -> Self {
            let len = width * height;
            Self {
                width,
                height,
                combined: vec![0; len],
                orange: vec![0; len],
                white_count: 0,
                orange_count: 0,
            }
        }

        fn analyze(&mut self, bgra: &[u8]) -> u64 {
            self.white_count = 0;
            self.orange_count = 0;

            let mut hash = 0xcbf29ce484222325_u64;

            for (i, px) in bgra.chunks_exact(4).enumerate() {
                let b = px[0];
                let g = px[1];
                let r = px[2];

                let orange = is_orange(r, g, b);
                let white = !orange && is_white(r, g, b);

                self.orange[i] = u8::from(orange);
                self.combined[i] = u8::from(orange || white);

                if orange {
                    self.orange_count += 1;
                }
                if white {
                    self.white_count += 1;
                }

                hash ^= self.combined[i] as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }

            hash
        }

        fn extract_text_images(&self) -> Vec<TextImage> {
            let mut orange_components = components(&self.orange, self.width, self.height, 5);
            orange_components.retain(|c| c.area >= 12 && c.height() >= 8 && c.width() >= 2);
            orange_components.sort_by_key(|c| (c.y0, c.x0));

            let mut out = Vec::new();
            let mut seen = Vec::<(usize, usize, usize, usize)>::new();

            for current in orange_components {
                if let Some(image) = self.extract_text_image_around(current) {
                    let key = image.source_bounds;
                    if !seen.iter().any(|b| rects_overlap(*b, key)) {
                        seen.push(key);
                        out.push(image);
                    }
                }
            }

            out
        }

        fn extract_text_image_around(&self, current: Component) -> Option<TextImage> {
            if current.height() < 8 || current.width() < 2 {
                return None;
            }

            let glyph_h = current.height() as f32;
            let all_components = components(&self.combined, self.width, self.height, 4);

            let mut accepted: Vec<Component> = all_components
                .into_iter()
                .filter(|c| {
                    let h = c.height() as f32;
                    let center_delta = (c.cy() - current.cy()).abs();
                    h >= glyph_h * 0.45 && h <= glyph_h * 1.65 && center_delta <= glyph_h * 0.55
                })
                .collect();

            if accepted.is_empty() {
                return None;
            }

            accepted.sort_by_key(|c| c.x0);

            let current_x = current.cx();
            let anchor_idx = accepted
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    let da = (a.cx() - current_x).abs();
                    let db = (b.cx() - current_x).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)?;

            let max_gap = (glyph_h * 1.15).max(8.0) as usize;
            let mut left_idx = anchor_idx;
            let mut right_idx = anchor_idx;

            while left_idx > 0 {
                let prev = accepted[left_idx - 1];
                let cur = accepted[left_idx];
                let gap = cur.x0.saturating_sub(prev.x1 + 1);
                if gap > max_gap {
                    break;
                }
                left_idx -= 1;
            }

            while right_idx + 1 < accepted.len() {
                let cur = accepted[right_idx];
                let next = accepted[right_idx + 1];
                let gap = next.x0.saturating_sub(cur.x1 + 1);
                if gap > max_gap {
                    break;
                }
                right_idx += 1;
            }

            let run = &accepted[left_idx..=right_idx];
            let mut x0 = run.iter().map(|c| c.x0).min()?;
            let mut y0 = run.iter().map(|c| c.y0).min()?;
            let mut x1 = run.iter().map(|c| c.x1).max()?;
            let mut y1 = run.iter().map(|c| c.y1).max()?;

            x0 = x0.saturating_sub(3);
            y0 = y0.saturating_sub(3);
            x1 = (x1 + 3).min(self.width - 1);
            y1 = (y1 + 3).min(self.height - 1);

            let scale = 4usize;
            let margin = 16usize;
            let src_w = x1 - x0 + 1;
            let src_h = y1 - y0 + 1;
            let out_w = src_w * scale + margin * 2;
            let out_h = src_h * scale + margin * 2;
            let mut pixels = vec![255u8; out_w * out_h];

            for sy in y0..=y1 {
                for sx in x0..=x1 {
                    let src_idx = sy * self.width + sx;
                    if self.combined[src_idx] == 0 {
                        continue;
                    }

                    let ox = margin + (sx - x0) * scale;
                    let oy = margin + (sy - y0) * scale;
                    for dy in 0..scale {
                        let row = (oy + dy) * out_w;
                        for dx in 0..scale {
                            pixels[row + ox + dx] = 0;
                        }
                    }
                }
            }

            let current_center_x = margin as f32 + (current.cx() - x0 as f32) * scale as f32;

            Some(TextImage {
                width: out_w,
                height: out_h,
                pixels,
                current_center_x,
                source_bounds: (x0, y0, x1, y1),
            })
        }
    }

    #[derive(Clone)]
    struct TextImage {
        width: usize,
        height: usize,
        pixels: Vec<u8>,
        current_center_x: f32,
        source_bounds: (usize, usize, usize, usize),
    }

    fn rects_overlap(a: (usize, usize, usize, usize), b: (usize, usize, usize, usize)) -> bool {
        let x_overlap = a.0 <= b.2 && b.0 <= a.2;
        let y_overlap = a.1 <= b.3 && b.1 <= a.3;
        x_overlap && y_overlap
    }

    #[derive(Clone, Debug)]
    struct OcrChar {
        ch: char,
        x0: i32,
    }

    #[derive(Clone, Debug)]
    struct OcrResult {
        word: String,
    }

    #[inline]
    fn is_orange(r: u8, g: u8, b: u8) -> bool {
        let r = r as i16;
        let g = g as i16;
        let b = b as i16;

        r >= 145
            && g >= 105
            && b <= 145
            && r - b >= 45
            && g - b >= 35
            && (r - g).abs() <= 70
            && r + 25 >= g
    }

    #[inline]
    fn is_white(r: u8, g: u8, b: u8) -> bool {
        let r = r as i16;
        let g = g as i16;
        let b = b as i16;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);

        min >= 140 && max - min <= 32
    }

    fn components(mask: &[u8], width: usize, height: usize, min_area: usize) -> Vec<Component> {
        let mut visited = vec![0u8; mask.len()];
        let mut stack = Vec::<usize>::new();
        let mut out = Vec::new();

        for start in 0..mask.len() {
            if mask[start] == 0 || visited[start] != 0 {
                continue;
            }

            visited[start] = 1;
            stack.clear();
            stack.push(start);

            let sx = start % width;
            let sy = start / width;
            let mut x0 = sx;
            let mut y0 = sy;
            let mut x1 = sx;
            let mut y1 = sy;
            let mut area = 0usize;

            while let Some(idx) = stack.pop() {
                area += 1;
                let x = idx % width;
                let y = idx / width;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);

                let nx0 = x.saturating_sub(1);
                let ny0 = y.saturating_sub(1);
                let nx1 = (x + 1).min(width - 1);
                let ny1 = (y + 1).min(height - 1);

                for ny in ny0..=ny1 {
                    for nx in nx0..=nx1 {
                        if nx == x && ny == y {
                            continue;
                        }
                        let ni = ny * width + nx;
                        if mask[ni] != 0 && visited[ni] == 0 {
                            visited[ni] = 1;
                            stack.push(ni);
                        }
                    }
                }
            }

            if area >= min_area {
                out.push(Component {
                    x0,
                    y0,
                    x1,
                    y1,
                    area,
                });
            }
        }

        out
    }

    fn make_pgm(image: &TextImage) -> Vec<u8> {
        let header = format!("P5\n{} {}\n255\n", image.width, image.height);
        let mut pgm = Vec::with_capacity(header.len() + image.pixels.len());
        pgm.extend_from_slice(header.as_bytes());
        pgm.extend_from_slice(&image.pixels);
        pgm
    }

    fn run_tesseract_makebox(tesseract: &Path, image: &TextImage) -> io::Result<OcrResult> {
        let pgm = make_pgm(image);

        let mut child = Command::new(tesseract)
            .args([
                "stdin",
                "stdout",
                "-l",
                "eng",
                "--psm",
                "7",
                "-c",
                "tessedit_char_whitelist=ABCDEFGHIJKLMNOPQRSTUVWXYZ",
                "makebox",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;

        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(&pgm)?;
        }
        drop(child.stdin.take());

        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "tesseract exited with {}",
                output.status
            )));
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut chars = Vec::<OcrChar>::new();

        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let Some(token) = parts.next() else {
                continue;
            };
            let Some(ch) = token.chars().next() else {
                continue;
            };
            if !ch.is_ascii_alphabetic() {
                continue;
            }

            let Some(x0) = parts.next().and_then(|v| v.parse::<i32>().ok()) else {
                continue;
            };
            let _bottom = parts.next();
            let Some(_x1) = parts.next().and_then(|v| v.parse::<i32>().ok()) else {
                continue;
            };

            chars.push(OcrChar {
                ch: ch.to_ascii_uppercase(),
                x0,
            });
        }

        chars.sort_by_key(|c| c.x0);
        let word: String = chars.iter().map(|c| c.ch).collect();
        Ok(OcrResult { word })
    }

    #[derive(Clone)]
    struct Glyph {
        x0: usize,
        x1: usize,
        bits: Vec<u8>,
    }

    #[derive(Clone)]
    struct GlyphTemplate {
        ch: char,
        bits: Vec<u8>,
    }

    struct TemplateDb {
        items: Vec<GlyphTemplate>,
    }

    #[derive(Clone, Debug)]
    struct FastResult {
        word: String,
        current_index: usize,
        min_similarity: f32,
    }

    impl TemplateDb {
        fn new() -> Self {
            let mut db = Self { items: Vec::new() };
            db.load_blob(SEED_TEMPLATES);
            db
        }

        fn load_blob(&mut self, bytes: &[u8]) {
            if bytes.len() < 8 || &bytes[..4] != b"TPR1" {
                return;
            }
            let count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
            let mut pos = 8usize;
            for _ in 0..count {
                if pos + 1 + TEMPLATE_LEN > bytes.len() {
                    break;
                }
                let ch = bytes[pos] as char;
                pos += 1;
                if ch.is_ascii_uppercase() {
                    let bits = bytes[pos..pos + TEMPLATE_LEN].to_vec();

                    let duplicate = self
                        .items
                        .iter()
                        .any(|t| t.ch == ch && glyph_similarity(&t.bits, &bits) > 0.995);
                    if !duplicate {
                        self.items.push(GlyphTemplate { ch, bits });
                    }
                }
                pos += TEMPLATE_LEN;
            }
        }

        fn known_letters(&self) -> String {
            let set: BTreeSet<char> = self.items.iter().map(|t| t.ch).collect();
            set.into_iter().collect()
        }

        fn recognize(&self, image: &TextImage) -> Option<FastResult> {
            let glyphs = segment_glyphs(image);
            if glyphs.len() < 3 || glyphs.len() > 20 || self.items.is_empty() {
                return None;
            }

            let mut word = String::with_capacity(glyphs.len());
            let mut min_similarity = 1.0f32;

            for glyph in &glyphs {
                let mut best_ch = None;
                let mut best = -1.0f32;
                let mut second = -1.0f32;
                let mut per_char = [0.0f32; 26];

                for template in &self.items {
                    let score = glyph_similarity(&glyph.bits, &template.bits);
                    let idx = (template.ch as u8 - b'A') as usize;
                    if score > per_char[idx] {
                        per_char[idx] = score;
                    }
                }

                for (idx, score) in per_char.into_iter().enumerate() {
                    if score > best {
                        second = best;
                        best = score;
                        best_ch = Some((b'A' + idx as u8) as char);
                    } else if score > second {
                        second = score;
                    }
                }

                if best < 0.70 || (best < 0.88 && best - second < 0.035) {
                    return None;
                }
                min_similarity = min_similarity.min(best);
                word.push(best_ch?);
            }

            let current_index = current_glyph_index(image, &glyphs)?;
            Some(FastResult {
                word,
                current_index,
                min_similarity,
            })
        }

        fn train_word(&mut self, image: &TextImage, word: &str) -> usize {
            let glyphs = segment_glyphs(image);
            let chars: Vec<char> = word
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            if glyphs.len() != chars.len() || chars.len() < 3 {
                return 0;
            }

            let mut added = 0usize;
            for (glyph, ch) in glyphs.iter().zip(chars) {
                let same: Vec<&GlyphTemplate> = self.items.iter().filter(|t| t.ch == ch).collect();
                let already = same
                    .iter()
                    .map(|t| glyph_similarity(&glyph.bits, &t.bits))
                    .fold(0.0f32, f32::max);
                if already > 0.975 || same.len() >= 8 {
                    continue;
                }
                self.items.push(GlyphTemplate {
                    ch,
                    bits: glyph.bits.clone(),
                });
                added += 1;
            }
            added
        }
    }

    fn text_image_hash(image: &TextImage) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for &px in &image.pixels {
            hash ^= (px < 128) as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    fn segment_glyphs(image: &TextImage) -> Vec<Glyph> {
        let mask: Vec<u8> = image.pixels.iter().map(|&px| u8::from(px < 128)).collect();
        let mut comps = components(&mask, image.width, image.height, 12);
        comps.retain(|c| c.height() >= 8 && c.width() >= 2);
        let max_h = comps.iter().map(|c| c.height()).max().unwrap_or(0);

        comps.retain(|c| c.height() * 100 >= max_h.saturating_mul(65));
        comps.sort_by_key(|c| c.x0);

        comps
            .into_iter()
            .map(|c| Glyph {
                x0: c.x0,
                x1: c.x1,
                bits: normalize_glyph(image, c.x0, c.y0, c.x1, c.y1),
            })
            .collect()
    }

    fn normalize_glyph(image: &TextImage, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<u8> {
        let src_w = x1 - x0 + 1;
        let src_h = y1 - y0 + 1;
        let scale =
            (TEMPLATE_INNER_W as f32 / src_w as f32).min(TEMPLATE_INNER_H as f32 / src_h as f32);
        let dst_w = ((src_w as f32 * scale).round() as usize).clamp(1, TEMPLATE_INNER_W);
        let dst_h = ((src_h as f32 * scale).round() as usize).clamp(1, TEMPLATE_INNER_H);
        let ox = (TEMPLATE_W - dst_w) / 2;
        let oy = (TEMPLATE_H - dst_h) / 2;
        let mut bits = vec![0u8; TEMPLATE_LEN];

        for dy in 0..dst_h {
            let sy = y0 + ((dy * src_h) / dst_h).min(src_h - 1);
            for dx in 0..dst_w {
                let sx = x0 + ((dx * src_w) / dst_w).min(src_w - 1);
                if image.pixels[sy * image.width + sx] < 128 {
                    bits[(oy + dy) * TEMPLATE_W + (ox + dx)] = 1;
                }
            }
        }
        bits
    }

    fn glyph_similarity(a: &[u8], b: &[u8]) -> f32 {
        let mut intersection = 0usize;
        let mut a_count = 0usize;
        let mut b_count = 0usize;
        for (&aa, &bb) in a.iter().zip(b) {
            if aa != 0 {
                a_count += 1;
            }
            if bb != 0 {
                b_count += 1;
            }
            if aa != 0 && bb != 0 {
                intersection += 1;
            }
        }
        let denom = a_count + b_count;
        if denom == 0 {
            0.0
        } else {
            (2 * intersection) as f32 / denom as f32
        }
    }

    fn current_glyph_index(image: &TextImage, glyphs: &[Glyph]) -> Option<usize> {
        let cx = image.current_center_x;
        glyphs
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let da = if cx < a.x0 as f32 {
                    a.x0 as f32 - cx
                } else if cx > a.x1 as f32 {
                    cx - a.x1 as f32
                } else {
                    0.0
                };
                let db = if cx < b.x0 as f32 {
                    b.x0 as f32 - cx
                } else if cx > b.x1 as f32 {
                    cx - b.x1 as f32
                } else {
                    0.0
                };
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(idx, _)| idx)
    }

    fn suffix_from(word: &str, current_index: usize) -> String {
        word.chars().skip(current_index).collect()
    }

    fn make_bitmap_info(width: i32, height: i32) -> BitmapInfo {
        BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as Dword,
                bi_width: width,
                bi_height: -height,
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: (width as u32)
                    .saturating_mul(height as u32)
                    .saturating_mul(4),
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        }
    }

    fn type_text(text: &str, key_delay_ms: u64) -> io::Result<()> {
        for ch in text.chars().filter(|ch| ch.is_ascii_alphabetic()) {
            let vk = ch.to_ascii_uppercase() as u32;
            let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as u16;
            if scan == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!("cannot map key {ch} to scan code"),
                ));
            }

            let down = Input {
                r#type: INPUT_KEYBOARD,
                _padding: 0,
                ki: KeyboardInput {
                    w_vk: 0,
                    w_scan: scan,
                    dw_flags: KEYEVENTF_SCANCODE,
                    time: 0,
                    dw_extra_info: 0,
                },
                _union_padding: 0,
            };
            let up = Input {
                r#type: INPUT_KEYBOARD,
                _padding: 0,
                ki: KeyboardInput {
                    w_vk: 0,
                    w_scan: scan,
                    dw_flags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dw_extra_info: 0,
                },
                _union_padding: 0,
            };
            let inputs = [down, up];
            let sent = unsafe {
                SendInput(
                    inputs.len() as Uint,
                    inputs.as_ptr(),
                    std::mem::size_of::<Input>() as i32,
                )
            };
            if sent != inputs.len() as Uint {
                return Err(io::Error::last_os_error());
            }
            if key_delay_ms != 0 {
                sleep(Duration::from_millis(key_delay_ms));
            }
        }
        Ok(())
    }

    fn search_rect_from_game(game: CaptureRect) -> CaptureRect {
        game
    }

    fn text_image_source_size(image: &TextImage) -> (usize, usize) {
        let (x0, y0, x1, y1) = image.source_bounds;
        (x1 - x0 + 1, y1 - y0 + 1)
    }

    fn likely_prompt_image(image: &TextImage) -> bool {
        let (w, h) = text_image_source_size(image);

        h >= 16 && h <= 100 && w >= (h.saturating_mul(3) / 2)
    }

    fn prompt_score(image: &TextImage) -> usize {
        let (w, h) = text_image_source_size(image);
        w.saturating_mul(h)
    }

    fn resolve_tesseract(explicit: Option<PathBuf>) -> io::Result<PathBuf> {
        if let Some(path) = explicit {
            if path.exists() || path.as_os_str() == "tesseract" {
                return Ok(path);
            }
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Tesseract not found: {}", path.display()),
            ));
        }

        let common = PathBuf::from(r"C:\Program Files\Tesseract-OCR\tesseract.exe");
        if common.exists() {
            return Ok(common);
        }

        if Command::new("tesseract")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(PathBuf::from("tesseract"));
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Tesseract not found. Install Tesseract OCR or provide --tesseract <path-to-tesseract.exe>",
        ))
    }

    pub fn run() -> io::Result<()> {
        let args = Args::default();
        let overlay = overlay::start("Automation Attack");
        overlay.set_target_fps(args.fps);
        overlay.set_lines([
            "status: loading embedded templates",
            "checking optional Tesseract OCR",
            "Ctrl+Shift+Q: stop",
        ]);

        let tesseract = match resolve_tesseract(None) {
            Ok(path) => {
                overlay.set_lines([
                    "Tesseract OCR: available".to_string(),
                    path.display().to_string(),
                    "launch: waiting for PLAY".to_string(),
                ]);
                Some(path)
            }
            Err(_) => {
                overlay.set_lines([
                    "Tesseract OCR not installed".to_string(),
                    "Install Tesseract OCR to use this tool".to_string(),
                    "quitting in 10s or Ctrl+Shift+Q".to_string(),
                ]);
                let started = Instant::now();
                while started.elapsed() < Duration::from_secs(10) {
                    if overlay.should_stop() {
                        break;
                    }
                    sleep(Duration::from_millis(100));
                }
                overlay.request_stop();
                return Ok(());
            }
        };
        overlay.set_lines(["launch: waiting for PLAY"]);
        let game_rect = match args.rect {
            Some(r) => r,
            None => {
                let Some(rect) =
                    launch_minigame_with_log(&overlay, LaunchProfile::Standard, &args.log_file)?
                else {
                    overlay.set_lines(["status: stopped before game area detection"]);
                    return Ok(());
                };
                CaptureRect {
                    left: rect.left,
                    top: rect.top,
                    width: rect.width,
                    height: rect.height,
                }
            }
        };
        let search_rect = search_rect_from_game(game_rect);

        let mut templates = TemplateDb::new();
        let _ = format!("\nScan FPS: {:.1}", args.fps);
        let _ = format!(
            "Search ROI: left={} top={} width={} height={}",
            search_rect.left, search_rect.top, search_rect.width, search_rect.height
        );
        let _ = format!("Fast templates: {}", templates.known_letters());
        if args.type_words {
            let _ = format!(
                "Input: ON, scan-code SendInput, key delay={} ms",
                args.key_delay_ms
            );
        } else {
            let _ = format!("Input: OFF (--no-type)");
        }

        let _ = format!("\nFAST matcher runs in-process on every candidate.");
        let _ = format!("Templates are embedded in the executable; no OCR install is required.");
        let _ = format!("Ctrl+C to exit.\n");

        let mut capture = ScreenCapture::new(search_rect.width, search_rect.height)?;
        let mut analyzer =
            ColorAnalyzer::new(search_rect.width as usize, search_rect.height as usize);
        let frame_delay = Duration::from_secs_f64(1.0 / args.fps.max(1.0));

        let (ocr_tx, ocr_rx) = if let Some(worker_tesseract) = tesseract.clone() {
            let (ocr_tx, ocr_job_rx) = sync_channel::<(u64, TextImage)>(1);
            let (ocr_result_tx, ocr_rx) =
                sync_channel::<(u64, TextImage, Result<OcrResult, String>, Duration)>(4);

            thread::spawn(move || {
                while let Ok((hash, image)) = ocr_job_rx.recv() {
                    let started = Instant::now();
                    let result = run_tesseract_makebox(&worker_tesseract, &image)
                        .map_err(|err| err.to_string());
                    let elapsed = started.elapsed();
                    if ocr_result_tx.send((hash, image, result, elapsed)).is_err() {
                        break;
                    }
                }
            });

            (Some(ocr_tx), Some(ocr_rx))
        } else {
            (None, None)
        };

        let mut frame_no = 0u64;
        let mut no_candidate_frames = 0u32;
        let mut last_typed_hash: Option<u64> = None;
        let mut last_ocr_requested_hash: Option<u64> = None;
        let mut last_status = String::new();
        let mut last_debug_save = Instant::now() - Duration::from_secs(10);

        loop {
            if overlay.should_stop() {
                break;
            }
            let frame_started = Instant::now();
            frame_no = frame_no.wrapping_add(1);

            let frame = capture.grab_bgra(search_rect)?;
            analyzer.analyze(frame);

            let images = analyzer.extract_text_images();
            let raw_candidates = images.len();
            let likely_candidates = images
                .iter()
                .filter(|img| likely_prompt_image(img) && segment_glyphs(img).len() >= 3)
                .count();

            let best = images
                .into_iter()
                .filter(|img| likely_prompt_image(img) && segment_glyphs(img).len() >= 3)
                .max_by_key(prompt_score);

            let current_hash = best.as_ref().map(text_image_hash);

            if let Some(image) = best.as_ref() {
                no_candidate_frames = 0;
                let hash = current_hash.unwrap();

                if args.debug && last_debug_save.elapsed() >= Duration::from_secs(1) {
                    let _ = fs::write("debug_text.pgm", make_pgm(image));
                    last_debug_save = Instant::now();
                }

                let fast_started = Instant::now();
                if let Some(result) = templates.recognize(image) {
                    let fast_us = fast_started.elapsed().as_micros();
                    let suffix = suffix_from(&result.word, result.current_index);
                    let current = result.word.chars().nth(result.current_index).unwrap_or('?');

                    let status = format!(
                        "FAST: {} | CURRENT={} | TYPE={} | score={:.1}% | {} us",
                        result.word,
                        current,
                        suffix,
                        result.min_similarity * 100.0,
                        fast_us
                    );
                    if status != last_status {
                        let _ = format!("{status}");
                        last_status = status;
                    }

                    if args.type_words && !suffix.is_empty() && last_typed_hash != Some(hash) {
                        type_text(&suffix, args.key_delay_ms)?;
                        last_typed_hash = Some(hash);
                        let _ = format!("  -> SENT: {suffix}");
                    }
                } else if let Some(ocr_tx) = ocr_tx.as_ref() {
                    if last_ocr_requested_hash != Some(hash) {
                        match ocr_tx.try_send((hash, image.clone())) {
                            Ok(()) => {
                                last_ocr_requested_hash = Some(hash);
                                let status = format!("OCR queued: hash={hash:016x}");
                                if status != last_status {
                                    last_status = status;
                                }
                            }
                            Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => {
                                let status = "OCR worker stopped".to_string();
                                if status != last_status {
                                    last_status = status;
                                }
                            }
                        }
                    }
                } else {
                    let status =
                        format!("install Tesseract OCR to learn missing word: {hash:016x}");
                    if status != last_status {
                        last_status = status;
                    }
                }
            } else {
                no_candidate_frames = no_candidate_frames.saturating_add(1);
                if no_candidate_frames >= 3 {
                    last_typed_hash = None;
                    last_ocr_requested_hash = None;
                    last_status.clear();
                }
            }

            if let Some(ocr_rx) = ocr_rx.as_ref() {
                while let Ok((hash, image, result, ocr_time)) = ocr_rx.try_recv() {
                    match result {
                        Ok(result) if !result.word.is_empty() => {
                            let added = templates.train_word(&image, &result.word);
                            let glyphs = segment_glyphs(&image);
                            let current_index = current_glyph_index(&image, &glyphs).unwrap_or(0);
                            let suffix = suffix_from(&result.word, current_index);
                            let current = result.word.chars().nth(current_index).unwrap_or('?');

                            let status = format!(
                                "OCR: {} CURRENT={} TYPE={} {:.1}ms learned +{}",
                                result.word,
                                current,
                                suffix,
                                ocr_time.as_secs_f64() * 1000.0,
                                added
                            );
                            if status != last_status {
                                last_status = status;
                            }

                            if args.type_words
                                && current_hash == Some(hash)
                                && last_typed_hash != Some(hash)
                                && !suffix.is_empty()
                            {
                                type_text(&suffix, args.key_delay_ms)?;
                                last_typed_hash = Some(hash);
                            }
                        }
                        Ok(_) => {
                            if args.debug {
                                last_status = format!(
                                    "OCR: nothing recognized ({:.1} ms)",
                                    ocr_time.as_secs_f64() * 1000.0
                                );
                            }
                        }
                        Err(err) => {
                            last_status = format!("OCR error: {err}");
                        }
                    }
                }
            }

            if args.debug {
                let _ = format!(
                    "FRAME {:06} | orange={:5} white={:5} candidates={}/{} | hash={}",
                    frame_no,
                    analyzer.orange_count,
                    analyzer.white_count,
                    likely_candidates,
                    raw_candidates,
                    current_hash
                        .map(|h| format!("{h:016x}"))
                        .unwrap_or_else(|| "-".to_string())
                );
            }

            let elapsed = frame_started.elapsed();
            let fps_now = 1.0 / elapsed.as_secs_f64().max(1e-6);
            let prompt_line = current_hash
                .map(|h| format!("prompt: hash={h:016x}"))
                .unwrap_or_else(|| "prompt: none".to_string());
            overlay.update(
                fps_now,
                args.fps,
                [
                    prompt_line,
                    format!("candidates: {likely_candidates}/{raw_candidates}"),
                    format!("templates: {}", templates.known_letters()),
                    if last_status.is_empty() {
                        "status: scanning".to_string()
                    } else {
                        last_status.clone()
                    },
                ],
            );
            if elapsed < frame_delay {
                sleep(frame_delay - elapsed);
            }
        }
        overlay.request_stop();
        Ok(())
    }
}

use super::{Tool, ToolResult};

#[derive(Default)]
pub struct AutomationAttack;

impl Tool for AutomationAttack {
    fn name(&self) -> &'static str {
        "automation_attack"
    }

    fn run(&mut self) -> ToolResult {
        #[cfg(target_os = "windows")]
        {
            return windows_app::run().map_err(Into::into);
        }

        #[cfg(not(target_os = "windows"))]
        {
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;

    type Hwnd = *mut c_void;
    type Hinstance = *mut c_void;
    type Hdc = *mut c_void;
    type Hbrush = *mut c_void;
    type Hgdiobj = *mut c_void;
    type Hpen = *mut c_void;
    type Hmenu = *mut c_void;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    const WIDTH: i32 = 420;
    const HEIGHT: i32 = 150;
    const FALLBACK_X: i32 = 30;
    const FALLBACK_Y: i32 = 30;

    const WM_HOTKEY: u32 = 0x0312;
    const WM_DESTROY: u32 = 0x0002;
    const WM_PAINT: u32 = 0x000F;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_TIMER: u32 = 0x0113;

    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_NOREPEAT: u32 = 0x4000;
    const HOTKEY_ID: i32 = 1;

    const WS_POPUP: u32 = 0x8000_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    const WS_EX_LAYERED: u32 = 0x0008_0000;
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    const SW_SHOW: i32 = 5;
    const LWA_COLORKEY: u32 = 0x0000_0001;
    const TRANSPARENT: i32 = 1;
    const BLACKNESS: u32 = 0x0000_0042;
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const TIMER_ID: usize = 1;
    const TIMER_INTERVAL_MS: u32 = 16;
    const EXIT_HINT: &str = "Press Ctrl + Shift + Q to exit";

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: Wparam,
        l_param: Lparam,
        time: u32,
        pt: Point,
        l_private: u32,
    }

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct PaintStruct {
        hdc: Hdc,
        erase: i32,
        paint: Rect,
        restore: i32,
        inc_update: i32,
        rgb_reserved: [u8; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        dw_low_date_time: u32,
        dw_high_date_time: u32,
    }

    type WndProc = unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult;
    type TimerProc = Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: Option<WndProc>,
        cls_extra: i32,
        wnd_extra: i32,
        instance: Hinstance,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: Hbrush,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
        fn GetWindowRect(hwnd: Hwnd, rect: *mut Rect) -> i32;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Hwnd,
            menu: Hmenu,
            instance: Hinstance,
            param: *const c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> Lresult;
        fn PostQuitMessage(code: i32);
        fn SetLayeredWindowAttributes(hwnd: Hwnd, color_key: u32, alpha: u8, flags: u32) -> i32;
        fn SetWindowDisplayAffinity(hwnd: Hwnd, affinity: u32) -> i32;
        fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Hdc;
        fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
        fn SetTimer(hwnd: Hwnd, id: usize, elapse_ms: u32, timer_proc: TimerProc) -> usize;
        fn KillTimer(hwnd: Hwnd, id: usize) -> i32;
        fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
        fn RegisterHotKey(hwnd: Hwnd, id: i32, modifiers: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: Hwnd, id: i32) -> i32;
        fn DestroyWindow(hwnd: Hwnd) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        fn SetTextColor(hdc: Hdc, color: u32) -> u32;
        fn TextOutW(hdc: Hdc, x: i32, y: i32, text: *const u16, len: i32) -> i32;
        fn PatBlt(hdc: Hdc, x: i32, y: i32, width: i32, height: i32, rop: u32) -> i32;
        fn CreateSolidBrush(color: u32) -> Hbrush;
        fn CreatePen(style: i32, width: i32, color: u32) -> Hpen;
        fn SelectObject(hdc: Hdc, object: Hgdiobj) -> Hgdiobj;
        fn DeleteObject(object: Hgdiobj) -> i32;
        fn Ellipse(hdc: Hdc, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        fn MoveToEx(hdc: Hdc, x: i32, y: i32, old: *mut Point) -> i32;
        fn LineTo(hdc: Hdc, x: i32, y: i32) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Hinstance;
        fn QueryPerformanceCounter(counter: *mut i64) -> i32;
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
        fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;
    }

    #[derive(Clone, Debug)]
    struct ClickMarker {
        x: i32,
        y: i32,
    }

    #[derive(Clone, Debug)]
    pub enum OverlayPrimitive {
        Rect {
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            color: u32,
        },
        Line {
            x0: i32,
            y0: i32,
            x1: i32,
            y1: i32,
            color: u32,
        },
        Cross {
            x: i32,
            y: i32,
            radius: i32,
            color: u32,
        },
    }

    #[derive(Clone, Debug)]
    struct OverlayState {
        title: String,
        script_fps: Option<f64>,
        target_fps: Option<f64>,
        lines: Vec<String>,
        click_markers: Vec<ClickMarker>,
        debug_primitives: Vec<OverlayPrimitive>,
    }

    impl OverlayState {
        fn new(title: &str) -> Self {
            Self {
                title: title.to_string(),
                script_fps: None,
                target_fps: None,
                lines: Vec::new(),
                click_markers: Vec::new(),
                debug_primitives: Vec::new(),
            }
        }
    }

    #[derive(Clone)]
    pub struct OverlayHandle {
        state: Arc<Mutex<OverlayState>>,
        stop_requested: Arc<AtomicBool>,
    }

    impl OverlayHandle {
        pub fn should_stop(&self) -> bool {
            self.stop_requested.load(Ordering::SeqCst)
        }

        pub fn request_stop(&self) {
            self.stop_requested.store(true, Ordering::SeqCst);
        }

        pub fn set_target_fps(&self, fps: f64) {
            if let Ok(mut state) = self.state.lock() {
                state.target_fps = Some(fps);
            }
        }

        pub fn set_lines<I, S>(&self, lines: I)
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            if let Ok(mut state) = self.state.lock() {
                state.lines = lines.into_iter().map(Into::into).take(4).collect();
            }
        }

        pub fn update<I, S>(&self, script_fps: f64, target_fps: f64, lines: I)
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
            if let Ok(mut state) = self.state.lock() {
                state.script_fps = Some(script_fps);
                state.target_fps = Some(target_fps);
                state.lines = lines.into_iter().map(Into::into).take(4).collect();
            }
        }

        pub fn mark_click(&self, x: i32, y: i32) {
            #[cfg(debug_assertions)]
            if let Ok(mut state) = self.state.lock() {
                state.click_markers.push(ClickMarker { x, y });
                if state.click_markers.len() > 64 {
                    let remove_count = state.click_markers.len() - 64;
                    state.click_markers.drain(0..remove_count);
                }
            }

            #[cfg(not(debug_assertions))]
            {
                let _ = (x, y);
            }
        }

        pub fn set_debug_primitives(&self, primitives: Vec<OverlayPrimitive>) {
            #[cfg(debug_assertions)]
            if let Ok(mut state) = self.state.lock() {
                state.debug_primitives = primitives;
            }

            #[cfg(not(debug_assertions))]
            {
                let _ = primitives;
            }
        }
    }

    #[derive(Clone, Copy, Default)]
    struct CpuTimes {
        idle: u64,
        kernel: u64,
        user: u64,
    }

    struct UiState {
        frequency: i64,
        cpu_last_update: i64,
        cpu_previous: CpuTimes,
        cpu: f64,
    }

    impl UiState {
        unsafe fn new() -> Self {
            let mut frequency = 0;
            let ok = unsafe { QueryPerformanceFrequency(&mut frequency) };
            assert!(ok != 0);
            assert!(frequency > 0);

            let now = unsafe { qpc_now() };
            let cpu_previous = unsafe { read_cpu_times() }.unwrap_or_default();

            Self {
                frequency,
                cpu_last_update: now,
                cpu_previous,
                cpu: 0.0,
            }
        }

        unsafe fn update_cpu(&mut self) {
            let now = unsafe { qpc_now() };
            let elapsed = (now - self.cpu_last_update) as f64 / self.frequency as f64;
            if elapsed < 0.25 {
                return;
            }

            self.cpu_last_update = now;
            let Some(current) = (unsafe { read_cpu_times() }) else {
                return;
            };

            let idle_delta = current.idle.saturating_sub(self.cpu_previous.idle);
            let kernel_delta = current.kernel.saturating_sub(self.cpu_previous.kernel);
            let user_delta = current.user.saturating_sub(self.cpu_previous.user);
            self.cpu_previous = current;

            let total = kernel_delta + user_delta;
            if total == 0 {
                return;
            }

            self.cpu = total.saturating_sub(idle_delta) as f64 / total as f64 * 100.0;
            self.cpu = self.cpu.clamp(0.0, 100.0);
        }
    }

    thread_local! {
        static UI_STATE: RefCell<Option<UiState>> = const { RefCell::new(None) };
    }

    static OVERLAY_STATE: OnceLock<Arc<Mutex<OverlayState>>> = OnceLock::new();
    static STOP_REQUESTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    static OVERLAY_STARTED: OnceLock<()> = OnceLock::new();

    pub fn start(title: &str) -> OverlayHandle {
        let state = OVERLAY_STATE
            .get_or_init(|| Arc::new(Mutex::new(OverlayState::new(title))))
            .clone();
        let stop_requested = STOP_REQUESTED
            .get_or_init(|| Arc::new(AtomicBool::new(false)))
            .clone();

        if let Ok(mut state) = state.lock() {
            state.title = title.to_string();
        }
        stop_requested.store(false, Ordering::SeqCst);

        if OVERLAY_STARTED.set(()).is_ok() {
            let window_title = title.to_string();
            thread::spawn(move || run_window(&window_title));
        }

        OverlayHandle {
            state,
            stop_requested,
        }
    }

    fn request_overlay_stop() {
        if let Some(stop_requested) = STOP_REQUESTED.get() {
            stop_requested.store(true, Ordering::SeqCst);
        }
    }

    unsafe fn qpc_now() -> i64 {
        let mut value = 0;
        unsafe {
            QueryPerformanceCounter(&mut value);
        }
        value
    }

    fn filetime_to_u64(time: FileTime) -> u64 {
        ((time.dw_high_date_time as u64) << 32) | time.dw_low_date_time as u64
    }

    unsafe fn read_cpu_times() -> Option<CpuTimes> {
        let mut idle = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();

        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }

        Some(CpuTimes {
            idle: filetime_to_u64(idle),
            kernel: filetime_to_u64(kernel),
            user: filetime_to_u64(user),
        })
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn default_bounds() -> (i32, i32, i32, i32) {
        unsafe {
            let dota_title = wide("Dota 2");
            let hwnd = FindWindowW(null(), dota_title.as_ptr());
            if hwnd.is_null() {
                return (FALLBACK_X, FALLBACK_Y, WIDTH, HEIGHT);
            }

            let mut rect: Rect = zeroed();
            if GetWindowRect(hwnd, &mut rect) == 0 {
                return (FALLBACK_X, FALLBACK_Y, WIDTH, HEIGHT);
            }

            (
                rect.left,
                rect.top,
                (rect.right - rect.left).max(WIDTH),
                (rect.bottom - rect.top).max(HEIGHT),
            )
        }
    }

    unsafe fn draw_text(hdc: Hdc, x: i32, y: i32, text: &str) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            TextOutW(hdc, x, y, utf16.as_ptr(), utf16.len() as i32);
        }
    }

    unsafe fn draw_click_marker(hdc: Hdc, x: i32, y: i32) {
        let brush = unsafe { CreateSolidBrush(0x0000_FF00) };
        if brush.is_null() {
            return;
        }

        let old = unsafe { SelectObject(hdc, brush as Hgdiobj) };
        unsafe {
            Ellipse(hdc, x - 5, y - 5, x + 6, y + 6);
        }
        if !old.is_null() {
            unsafe {
                SelectObject(hdc, old);
            }
        }
        unsafe {
            DeleteObject(brush as Hgdiobj);
        }
    }

    unsafe fn draw_line(hdc: Hdc, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, width: i32) {
        let pen = unsafe { CreatePen(0, width.max(1), color) };
        if pen.is_null() {
            return;
        }
        let old = unsafe { SelectObject(hdc, pen as Hgdiobj) };
        unsafe {
            MoveToEx(hdc, x0, y0, null_mut());
            LineTo(hdc, x1, y1);
        }
        if !old.is_null() {
            unsafe {
                SelectObject(hdc, old);
            }
        }
        unsafe {
            DeleteObject(pen as Hgdiobj);
        }
    }

    unsafe fn draw_primitive(hdc: Hdc, primitive: &OverlayPrimitive, window_rect: &Rect) {
        match *primitive {
            OverlayPrimitive::Rect { x, y, w, h, color } => {
                let x = x - window_rect.left;
                let y = y - window_rect.top;
                unsafe {
                    draw_line(hdc, x, y, x + w, y, color, 3);
                    draw_line(hdc, x + w, y, x + w, y + h, color, 3);
                    draw_line(hdc, x + w, y + h, x, y + h, color, 3);
                    draw_line(hdc, x, y + h, x, y, color, 3);
                }
            }
            OverlayPrimitive::Line {
                x0,
                y0,
                x1,
                y1,
                color,
            } => unsafe {
                draw_line(
                    hdc,
                    x0 - window_rect.left,
                    y0 - window_rect.top,
                    x1 - window_rect.left,
                    y1 - window_rect.top,
                    color,
                    3,
                );
            },
            OverlayPrimitive::Cross {
                x,
                y,
                radius,
                color,
            } => {
                let x = x - window_rect.left;
                let y = y - window_rect.top;
                unsafe {
                    draw_line(hdc, x - radius, y, x + radius, y, color, 3);
                    draw_line(hdc, x, y - radius, x, y + radius, color, 3);
                }
            }
        }
    }

    fn click_markers() -> Vec<ClickMarker> {
        let Some(state) = OVERLAY_STATE.get() else {
            return Vec::new();
        };

        let Ok(state) = state.lock() else {
            return Vec::new();
        };

        state.click_markers.clone()
    }

    fn debug_primitives() -> Vec<OverlayPrimitive> {
        let Some(state) = OVERLAY_STATE.get() else {
            return Vec::new();
        };

        let Ok(state) = state.lock() else {
            return Vec::new();
        };

        state.debug_primitives.clone()
    }

    fn telemetry_lines(cpu: f64) -> Vec<String> {
        let Some(state) = OVERLAY_STATE.get() else {
            return vec![format!("CPU: {cpu:.1}%")];
        };

        let Ok(state) = state.lock() else {
            return vec![format!("CPU: {cpu:.1}%")];
        };

        let fps = match (state.script_fps, state.target_fps) {
            (Some(actual), Some(target)) => format!("Script FPS: {actual:.1} / {target:.1}"),
            (Some(actual), None) => format!("Script FPS: {actual:.1}"),
            (None, Some(target)) => format!("Script FPS: - / {target:.1}"),
            (None, None) => "Script FPS: -".to_string(),
        };

        let mut lines = vec![
            state.title.clone(),
            fps,
            format!("CPU: {cpu:.1}%"),
            EXIT_HINT.to_string(),
        ];
        lines.extend(state.lines.iter().cloned());
        lines.truncate(7);
        lines
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        match msg {
            WM_TIMER => {
                UI_STATE.with(|state| {
                    if let Some(state) = state.borrow_mut().as_mut() {
                        unsafe {
                            state.update_cpu();
                        }
                    }
                });

                unsafe {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_HOTKEY => {
                if wparam == HOTKEY_ID as usize {
                    request_overlay_stop();
                    unsafe {
                        DestroyWindow(hwnd);
                    }
                }
                0
            }
            WM_PAINT => {
                let mut ps: PaintStruct = unsafe { zeroed() };
                let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
                let mut window_rect: Rect = unsafe { zeroed() };
                let has_window_rect = unsafe { GetWindowRect(hwnd, &mut window_rect) } != 0;
                let paint_width = if has_window_rect {
                    (window_rect.right - window_rect.left).max(WIDTH)
                } else {
                    WIDTH
                };
                let paint_height = if has_window_rect {
                    (window_rect.bottom - window_rect.top).max(HEIGHT)
                } else {
                    HEIGHT
                };

                unsafe {
                    PatBlt(hdc, 0, 0, paint_width, paint_height, BLACKNESS);
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, 0x0000_FF00);
                }

                let cpu = UI_STATE.with(|state| state.borrow().as_ref().map_or(0.0, |s| s.cpu));
                for (index, line) in telemetry_lines(cpu).iter().enumerate() {
                    unsafe {
                        draw_text(hdc, 15, 12 + index as i32 * 18, line);
                    }
                }

                if has_window_rect {
                    for primitive in debug_primitives() {
                        unsafe {
                            draw_primitive(hdc, &primitive, &window_rect);
                        }
                    }

                    for marker in click_markers() {
                        let x = marker.x - window_rect.left;
                        let y = marker.y - window_rect.top;
                        if x >= 0 && y >= 0 && x < paint_width && y < paint_height {
                            unsafe {
                                draw_click_marker(hdc, x, y);
                            }
                        }
                    }
                }

                unsafe {
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_ERASEBKGND => 1,
            WM_DESTROY => {
                request_overlay_stop();
                unsafe {
                    KillTimer(hwnd, TIMER_ID);
                    UnregisterHotKey(hwnd, HOTKEY_ID);
                    PostQuitMessage(0);
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn run_window(title: &str) {
        unsafe {
            UI_STATE.with(|state| {
                *state.borrow_mut() = Some(UiState::new());
            });

            let instance = GetModuleHandleW(null());
            assert!(!instance.is_null(), "GetModuleHandleW failed");

            let class_name = wide("MidnightRunToolsOverlay");
            let title = wide(title);

            let wc = WndClassW {
                style: 0,
                wnd_proc: Some(wnd_proc),
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: null_mut(),
                cursor: null_mut(),
                background: null_mut(),
                menu_name: null(),
                class_name: class_name.as_ptr(),
            };

            let atom = RegisterClassW(&wc);
            assert!(atom != 0, "RegisterClassW failed");

            let (x, y, width, height) = default_bounds();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                width,
                height,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null(), "CreateWindowExW failed");

            let ok = RegisterHotKey(
                hwnd,
                HOTKEY_ID,
                MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                b'Q' as u32,
            );
            assert!(ok != 0, "RegisterHotKey failed");

            let ok = SetLayeredWindowAttributes(hwnd, 0x0000_0000, 255, LWA_COLORKEY);
            assert!(ok != 0, "SetLayeredWindowAttributes failed");

            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);

            let timer = SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None);
            assert!(timer != 0, "SetTimer failed");

            ShowWindow(hwnd, SW_SHOW);

            let mut msg: Msg = zeroed();
            loop {
                let result = GetMessageW(&mut msg, null_mut(), 0, 0);
                if result <= 0 {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    #[derive(Clone, Default)]
    pub struct OverlayHandle;

    #[derive(Clone, Debug)]
    pub enum OverlayPrimitive {
        Rect {
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            color: u32,
        },
        Line {
            x0: i32,
            y0: i32,
            x1: i32,
            y1: i32,
            color: u32,
        },
        Cross {
            x: i32,
            y: i32,
            radius: i32,
            color: u32,
        },
    }

    impl OverlayHandle {
        pub fn should_stop(&self) -> bool {
            false
        }

        pub fn request_stop(&self) {}

        pub fn set_target_fps(&self, _fps: f64) {}

        pub fn set_lines<I, S>(&self, _lines: I)
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
        }

        pub fn update<I, S>(&self, _script_fps: f64, _target_fps: f64, _lines: I)
        where
            I: IntoIterator<Item = S>,
            S: Into<String>,
        {
        }

        pub fn mark_click(&self, _x: i32, _y: i32) {}

        pub fn set_debug_primitives(&self, _primitives: Vec<OverlayPrimitive>) {}
    }

    pub fn start(_title: &str) -> OverlayHandle {
        OverlayHandle
    }
}

pub use imp::{OverlayHandle, OverlayPrimitive, start};

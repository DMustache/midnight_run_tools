use std::ffi::c_void;
use std::io;

pub type Bool = i32;
pub type Dword = u32;
pub type Long = i32;
pub type Uint = u32;
pub type Word = u16;
pub type Hwnd = *mut c_void;
pub type Hdc = *mut c_void;
pub type Hbitmap = *mut c_void;
pub type Hgdiobj = *mut c_void;

pub const SRCCOPY: Dword = 0x00CC0020;
pub const DIB_RGB_COLORS: Uint = 0;
pub const BI_RGB: Dword = 0;
pub const INPUT_KEYBOARD: Dword = 1;
pub const KEYEVENTF_KEYUP: Dword = 0x0002;
pub const KEYEVENTF_SCANCODE: Dword = 0x0008;
pub const MOUSEEVENTF_LEFTDOWN: Dword = 0x0002;
pub const MOUSEEVENTF_LEFTUP: Dword = 0x0004;
pub const MAPVK_VK_TO_VSC: Uint = 0;
pub const VK_A: Word = 0x41;
pub const VK_D: Word = 0x44;
pub const VK_SPACE: Word = 0x20;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: Long,
    pub y: Long,
}

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

#[repr(C)]
pub struct BitmapInfoHeader {
    pub bi_size: Dword,
    pub bi_width: Long,
    pub bi_height: Long,
    pub bi_planes: Word,
    pub bi_bit_count: Word,
    pub bi_compression: Dword,
    pub bi_size_image: Dword,
    pub bi_x_pels_per_meter: Long,
    pub bi_y_pels_per_meter: Long,
    pub bi_clr_used: Dword,
    pub bi_clr_important: Dword,
}

#[repr(C)]
pub struct BitmapInfo {
    pub bmi_header: BitmapInfoHeader,
    pub bmi_colors: [Dword; 1],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KeyBdInput {
    pub w_vk: Word,
    pub w_scan: Word,
    pub dw_flags: Dword,
    pub time: Dword,
    pub dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Input {
    pub r#type: Dword,
    pub _pad: Dword,
    pub ki: KeyBdInput,
    pub _union_pad: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct WinRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(crate) fn GetCursorPos(lp_point: *mut Point) -> Bool;
    pub(crate) fn SetCursorPos(x: i32, y: i32) -> Bool;
    pub(crate) fn mouse_event(
        dw_flags: Dword,
        dx: Dword,
        dy: Dword,
        dw_data: Dword,
        dw_extra_info: usize,
    );
    pub(crate) fn GetDC(h_wnd: Hwnd) -> Hdc;
    pub(crate) fn ReleaseDC(h_wnd: Hwnd, h_dc: Hdc) -> i32;
    pub(crate) fn SendInput(c_inputs: Uint, p_inputs: *const Input, cb_size: i32) -> Uint;
    pub(crate) fn MapVirtualKeyW(code: Uint, map_type: Uint) -> Uint;
    pub(crate) fn FindWindowW(class_name: *const u16, window_name: *const u16) -> Hwnd;
    pub(crate) fn GetWindowRect(hwnd: Hwnd, rect: *mut WinRect) -> Bool;
    pub(crate) fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    pub(crate) fn CreateCompatibleDC(hdc: Hdc) -> Hdc;
    pub(crate) fn CreateCompatibleBitmap(hdc: Hdc, cx: i32, cy: i32) -> Hbitmap;
    pub(crate) fn CreateDIBSection(
        hdc: Hdc,
        pbmi: *const BitmapInfo,
        usage: Uint,
        ppv_bits: *mut *mut c_void,
        section: *mut c_void,
        offset: Dword,
    ) -> Hbitmap;
    pub(crate) fn SelectObject(hdc: Hdc, h: Hgdiobj) -> Hgdiobj;
    pub(crate) fn BitBlt(
        hdc: Hdc,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        hdc_src: Hdc,
        x1: i32,
        y1: i32,
        rop: Dword,
    ) -> Bool;
    pub(crate) fn GetDIBits(
        hdc: Hdc,
        hbm: Hbitmap,
        start: Uint,
        c_lines: Uint,
        lpv_bits: *mut c_void,
        lpbmi: *mut BitmapInfo,
        usage: Uint,
    ) -> i32;
    pub(crate) fn DeleteObject(ho: Hgdiobj) -> Bool;
    pub(crate) fn DeleteDC(hdc: Hdc) -> Bool;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(Dword) -> Bool>,
        add: Bool,
    ) -> Bool;
}

pub struct ScreenCapture {
    screen_dc: Hdc,
    mem_dc: Hdc,
    bitmap: Hbitmap,
    old_obj: Hgdiobj,
    width: i32,
    height: i32,
    buf: Vec<u8>,
}

pub fn find_window_rect(title: &str) -> Option<Rect> {
    let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            return None;
        }

        let mut rect = WinRect::default();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }

        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return None;
        }

        Some(Rect {
            left: rect.left,
            top: rect.top,
            width,
            height,
        })
    }
}

pub fn primary_screen_rect() -> Rect {
    unsafe {
        Rect {
            left: 0,
            top: 0,
            width: GetSystemMetrics(SM_CXSCREEN).max(1),
            height: GetSystemMetrics(SM_CYSCREEN).max(1),
        }
    }
}

impl ScreenCapture {
    pub fn new(width: i32, height: i32) -> io::Result<Self> {
        unsafe {
            let screen_dc = GetDC(std::ptr::null_mut());
            if screen_dc.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_null() {
                ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err(io::Error::last_os_error());
            }
            let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
            if bitmap.is_null() {
                DeleteDC(mem_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
                return Err(io::Error::last_os_error());
            }
            let old_obj = SelectObject(mem_dc, bitmap as Hgdiobj);
            Ok(Self {
                screen_dc,
                mem_dc,
                bitmap,
                old_obj,
                width,
                height,
                buf: vec![0; (width * height * 4) as usize],
            })
        }
    }

    pub fn grab_bgra(&mut self, rect: Rect) -> io::Result<&[u8]> {
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
            let mut info = BitmapInfo {
                bmi_header: BitmapInfoHeader {
                    bi_size: std::mem::size_of::<BitmapInfoHeader>() as Dword,
                    bi_width: self.width,
                    bi_height: -self.height,
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: BI_RGB,
                    bi_size_image: (self.width * self.height * 4) as Dword,
                    bi_x_pels_per_meter: 0,
                    bi_y_pels_per_meter: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                bmi_colors: [0],
            };
            let got = GetDIBits(
                self.mem_dc,
                self.bitmap,
                0,
                self.height as Uint,
                self.buf.as_mut_ptr() as *mut c_void,
                &mut info,
                DIB_RGB_COLORS,
            );
            if got == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(&self.buf)
        }
    }
}

impl Drop for ScreenCapture {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem_dc, self.old_obj);
            DeleteObject(self.bitmap as Hgdiobj);
            DeleteDC(self.mem_dc);
            ReleaseDC(std::ptr::null_mut(), self.screen_dc);
        }
    }
}

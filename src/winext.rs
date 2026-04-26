use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

pub fn hwnd_from(frame: &eframe::Frame) -> Option<HWND> {
    let handle = frame.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut std::ffi::c_void)),
        _ => None,
    }
}

pub fn set_skip_taskbar(hwnd: HWND, skip: bool) {
    unsafe {
        let cur = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let tool = WS_EX_TOOLWINDOW.0 as isize;
        let app = WS_EX_APPWINDOW.0 as isize;
        let new_style = if skip {
            (cur | tool) & !app
        } else {
            (cur & !tool) | app
        };
        if new_style != cur {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
        }
    }
}

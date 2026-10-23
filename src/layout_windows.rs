//! Own an invisible top-level window so sent and posted display notifications
//! are observed even when WayX has no native application windows.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use windows_sys::Win32::{
    Foundation::*, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*,
};
struct Context {
    dirty: Arc<AtomicBool>,
    wake: crate::notify::UiWake,
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: Windows supplies CREATESTRUCTW and the creator keeps Context
        // alive until DestroyWindow and the message loop have both completed.
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    if matches!(message, WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED) {
        let context = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Context };
        if let Some(context) = unsafe { context.as_ref() } {
            context.dirty.store(true, Ordering::Release);
            (context.wake)();
        }
    }
    if message == WM_DESTROY {
        unsafe {
            PostQuitMessage(0);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
pub(crate) struct Watch {
    hwnd: usize,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Watch {
    pub fn new(dirty: Arc<AtomicBool>, wake: crate::notify::UiWake) -> anyhow::Result<Self> {
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("wayx-host-display".into())
            .spawn(move || {
                let name: Vec<u16> = "WayXHostDisplayNotifications\0".encode_utf16().collect();
                let mut context = Box::new(Context { dirty, wake });
                // SAFETY: this thread owns registration, window, context and pump.
                unsafe {
                    let instance = GetModuleHandleW(std::ptr::null());
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(window_proc),
                        hInstance: instance,
                        lpszClassName: name.as_ptr(),
                        ..std::mem::zeroed()
                    };
                    if RegisterClassW(&class) == 0 {
                        let _ = send.send(Err(std::io::Error::last_os_error()));
                        return;
                    }
                    // A message-only window would not receive display broadcasts.
                    let hwnd = CreateWindowExW(
                        0,
                        name.as_ptr(),
                        name.as_ptr(),
                        WS_OVERLAPPED,
                        0,
                        0,
                        0,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        instance,
                        (&mut *context as *mut Context).cast(),
                    );
                    if hwnd.is_null() {
                        let _ = send.send(Err(std::io::Error::last_os_error()));
                        UnregisterClassW(name.as_ptr(), instance);
                        return;
                    }
                    if send.send(Ok(hwnd as usize)).is_err() {
                        DestroyWindow(hwnd);
                        UnregisterClassW(name.as_ptr(), instance);
                        return;
                    }
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    if IsWindow(hwnd) != 0 {
                        DestroyWindow(hwnd);
                    }
                    UnregisterClassW(name.as_ptr(), instance);
                }
            })?;
        let hwnd = receive.recv()??;
        Ok(Self {
            hwnd,
            worker: Some(worker),
        })
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: this handle belongs to our still-running notification thread.
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

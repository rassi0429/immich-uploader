use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    pub fn try_acquire() -> Option<Self> {
        unsafe {
            let result = CreateMutexW(None, true, w!("Local\\immich-auto-uploader-instance"));
            let handle = match result {
                Ok(h) => h,
                Err(_) => return None,
            };
            let last_err = GetLastError();
            if last_err == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return None;
            }
            Some(Self { handle })
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

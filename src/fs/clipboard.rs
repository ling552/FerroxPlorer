//! 文本剪贴板写入：供属性对话框「复制哈希值」等使用

/// 将文本写入系统剪贴板，成功返回 true
#[cfg(windows)]
pub fn set_text(text: &str) -> bool {
    use std::ffi::c_void;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };

    // CF_UNICODETEXT 标准剪贴板格式编号
    const CF_UNICODETEXT: u32 = 13;

    // 以 NUL 结尾的 UTF-16
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();

        let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if hmem.is_null() {
            CloseClipboard();
            return false;
        }
        let dst = GlobalLock(hmem) as *mut u16;
        if dst.is_null() {
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
        GlobalUnlock(hmem);

        // 所有权移交给系统；成功后不可再释放 hmem
        let ok = !SetClipboardData(CF_UNICODETEXT, hmem as *mut c_void).is_null();
        CloseClipboard();
        ok
    }
}

#[cfg(not(windows))]
pub fn set_text(_text: &str) -> bool {
    false
}

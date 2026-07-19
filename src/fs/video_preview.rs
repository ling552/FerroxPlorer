//! Quick Look 视频内嵌播放：Media Foundation MFPlay 在宿主窗口的子窗口中
//! 渲染视频画面并输出音频，与预览浮层的内容区对齐覆盖。
//!
//! 生命周期由 UI 线程管理（thread_local）：打开视频预览时创建子窗口 + 播放器，
//! 关闭预览（空格/Esc/点击遮罩/切换文件）时停止播放并销毁子窗口。

/// 启动播放：`parent` 为主窗口 HWND（isize），`rect` 为子窗口在父窗口客户区内的
/// 物理像素位置 (x, y, w, h)，`path` 为视频文件完整路径。返回是否成功启动。
#[cfg(windows)]
pub fn start(parent: isize, rect: (i32, i32, i32, i32), path: &str) -> bool {
    win_impl::start(parent, rect, path)
}

#[cfg(not(windows))]
pub fn start(_parent: isize, _rect: (i32, i32, i32, i32), _path: &str) -> bool {
    false
}

/// 停止播放并销毁子窗口（未在播放时为空操作）。
pub fn stop() {
    #[cfg(windows)]
    win_impl::stop();
}

#[cfg(windows)]
mod win_impl {
    use std::cell::RefCell;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Media::MediaFoundation::{
        IMFPMediaPlayer, MFPCreateMediaPlayer, MFP_OPTION_NONE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_CLIPSIBLINGS,
        WS_VISIBLE,
    };

    /// STATIC 控件的 SS_BLACKRECT 样式：黑色矩形填充，免自绘视频底色
    const SS_BLACKRECT_STYLE: u32 = 0x0004;

    thread_local! {
        // (播放器, 子窗口句柄)：仅 UI 线程访问
        static ACTIVE: RefCell<Option<(IMFPMediaPlayer, isize)>> = const { RefCell::new(None) };
    }

    pub fn start(parent: isize, rect: (i32, i32, i32, i32), path: &str) -> bool {
        // 先停掉上一次播放（切换视频/重复打开）
        stop();

        let class: Vec<u16> = "STATIC".encode_utf16().chain(std::iter::once(0)).collect();
        let url: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            // 黑底子窗口承载视频画面（SS_BLACKRECT 静态控件免自绘背景）
            let style = WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WINDOW_STYLE(SS_BLACKRECT_STYLE);
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                style,
                rect.0,
                rect.1,
                rect.2,
                rect.3,
                Some(HWND(parent as *mut core::ffi::c_void)),
                None,
                None,
                None,
            ) {
                Ok(h) => h,
                Err(_) => return false,
            };

            // MFPlay：URL + 立即播放 + 视频渲染到子窗口（音频自动路由默认设备）
            let mut player: Option<IMFPMediaPlayer> = None;
            let created = MFPCreateMediaPlayer(
                PCWSTR(url.as_ptr()),
                true,
                MFP_OPTION_NONE,
                None,
                Some(hwnd),
                Some(&mut player),
            );
            let Some(player) = created.ok().and(player) else {
                let _ = DestroyWindow(hwnd);
                return false;
            };
            ACTIVE.with(|a| *a.borrow_mut() = Some((player, hwnd.0 as isize)));
        }
        true
    }

    pub fn stop() {
        ACTIVE.with(|a| {
            if let Some((player, hwnd)) = a.borrow_mut().take() {
                unsafe {
                    let _ = player.Stop();
                    let _ = player.Shutdown();
                    let _ = DestroyWindow(HWND(hwnd as *mut core::ffi::c_void));
                }
            }
        });
    }
}

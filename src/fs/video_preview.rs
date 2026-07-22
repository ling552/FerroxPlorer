//! Quick Look 视频内嵌播放：Media Foundation MFPlay 在宿主窗口的子窗口中
//! 渲染视频画面并输出音频，与预览浮层的内容区对齐覆盖。
//!
//! 生命周期由 UI 线程管理（thread_local）：打开视频预览时创建子窗口 + 播放器，
//! 关闭预览（空格/Esc/点击遮罩/切换文件）时停止播放并销毁子窗口。
//!
//! 性能：媒体源解析（容器嗅探/解码器加载）经 IMFPMediaPlayerCallback 异步进行
//! ——MFPCreateMediaPlayer 不传 URL 立即返回，CreateMediaItemFromURL(fSync=FALSE)
//! 后台解析，MEDIAITEM_CREATED → SetMediaItem → MEDIAITEM_SET → Play。
//! 旧实现同步传 URL 创建会阻塞 UI 线程直至媒体源就绪，大文件/机械盘上打开明显卡顿。
//! MFPlay 通过隐藏窗口把事件序列化回创建线程（UI 线程）的消息循环，回调内可安全
//! 调用播放器方法；分辨率就绪后经 `ready` 回调上报（预览卡片按视频宽高比自适应）。

/// 启动播放：`parent` 为主窗口 HWND（isize），`rect` 为子窗口在父窗口客户区内的
/// 物理像素位置 (x, y, w, h)，`path` 为视频文件完整路径。
/// `ready(视频宽, 视频高)` 在媒体项就绪、开始播放时回调（UI 线程消息循环内）。
/// 返回是否成功启动异步加载。
#[cfg(windows)]
pub fn start(
    parent: isize,
    rect: (i32, i32, i32, i32),
    path: &str,
    ready: Box<dyn Fn(u32, u32) + Send>,
) -> bool {
    win_impl::start(parent, rect, path, ready)
}

#[cfg(not(windows))]
pub fn start(
    _parent: isize,
    _rect: (i32, i32, i32, i32),
    _path: &str,
    _ready: Box<dyn Fn(u32, u32) + Send>,
) -> bool {
    false
}

/// 停止播放并销毁子窗口（未在播放时为空操作）。
pub fn stop() {
    #[cfg(windows)]
    win_impl::stop();
}

/// 移动/缩放播放子窗口到新的物理像素矩形（未在播放时为空操作）。
/// 预览卡片按视频原生宽高比自适应大小后，由 UI 线程调用对齐子窗口。
#[cfg(windows)]
pub fn reposition(rect: (i32, i32, i32, i32)) {
    win_impl::reposition(rect);
}

#[cfg(not(windows))]
pub fn reposition(_rect: (i32, i32, i32, i32)) {}

#[cfg(windows)]
mod win_impl {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows::core::{implement, PCWSTR};
    use windows::Win32::Foundation::{HWND, SIZE};
    use windows::Win32::Media::MediaFoundation::{
        IMFPMediaPlayer, IMFPMediaPlayerCallback, IMFPMediaPlayerCallback_Impl,
        MFPCreateMediaPlayer, MFP_EVENT_HEADER, MFP_EVENT_TYPE_MEDIAITEM_CREATED,
        MFP_EVENT_TYPE_MEDIAITEM_SET, MFP_MEDIAITEM_CREATED_EVENT, MFP_MEDIAITEM_SET_EVENT,
        MFP_OPTION_NONE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, MoveWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD,
        WS_CLIPSIBLINGS, WS_VISIBLE,
    };

    /// STATIC 控件的 SS_BLACKRECT 样式：黑色矩形填充，免自绘视频底色
    const SS_BLACKRECT_STYLE: u32 = 0x0004;

    thread_local! {
        // (播放器, 子窗口句柄)：仅 UI 线程访问
        static ACTIVE: RefCell<Option<(IMFPMediaPlayer, isize)>> = const { RefCell::new(None) };
    }

    /// 播放代次：每次 start/stop 自增。异步事件携带发起时代次（dwUserData），
    /// 回调内比对当前代次——快速切换视频时丢弃迟到的旧媒体项，防止画面串台。
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    /// MFPlay 事件回调：媒体项异步创建完成 → 装载；装载完成 → 播放 + 上报分辨率。
    /// MFPlay 把事件序列化回创建线程（UI 线程）的消息循环，方法内可直接调用播放器。
    #[implement(IMFPMediaPlayerCallback)]
    struct PlayerCallback {
        generation: u64,
        ready: Box<dyn Fn(u32, u32) + Send>,
    }

    impl IMFPMediaPlayerCallback_Impl for PlayerCallback_Impl {
        fn OnMediaPlayerEvent(&self, peventheader: *const MFP_EVENT_HEADER) {
            unsafe {
                if peventheader.is_null() {
                    return;
                }
                let header = &*peventheader;
                // 本回调所属播放已被停止/替换：忽略一切迟到事件
                if self.generation != GENERATION.load(Ordering::SeqCst) {
                    return;
                }
                let Some(player) = header.pMediaPlayer.as_ref() else {
                    return;
                };
                match header.eEventType {
                    t if t == MFP_EVENT_TYPE_MEDIAITEM_CREATED => {
                        if header.hrEvent.is_err() {
                            return;
                        }
                        let ev = &*(peventheader as *const MFP_MEDIAITEM_CREATED_EVENT);
                        // dwUserData 携带发起时代次，双重校验防串台
                        if ev.dwUserData as u64 != self.generation {
                            return;
                        }
                        if let Some(item) = ev.pMediaItem.as_ref() {
                            let _ = player.SetMediaItem(item);
                        }
                    }
                    t if t == MFP_EVENT_TYPE_MEDIAITEM_SET => {
                        if header.hrEvent.is_err() {
                            return;
                        }
                        let _ev = &*(peventheader as *const MFP_MEDIAITEM_SET_EVENT);
                        let _ = player.Play();
                        // 上报原生分辨率（供预览卡片按宽高比自适应）
                        let mut native = SIZE::default();
                        let mut ar = SIZE::default();
                        if player
                            .GetNativeVideoSize(Some(&mut native), Some(&mut ar))
                            .is_ok()
                            && native.cx > 0
                            && native.cy > 0
                        {
                            (self.ready)(native.cx as u32, native.cy as u32);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn start(
        parent: isize,
        rect: (i32, i32, i32, i32),
        path: &str,
        ready: Box<dyn Fn(u32, u32) + Send>,
    ) -> bool {
        // 先停掉上一次播放（切换视频/重复打开）
        stop();
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

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

            // 播放器本体创建很快（不传 URL 不解析媒体）；媒体源解析走异步回调
            let callback: IMFPMediaPlayerCallback = PlayerCallback { generation, ready }.into();
            let mut player: Option<IMFPMediaPlayer> = None;
            let created = MFPCreateMediaPlayer(
                PCWSTR::null(),
                false,
                MFP_OPTION_NONE,
                Some(&callback),
                Some(hwnd),
                Some(&mut player),
            );
            let Some(player) = created.ok().and(player) else {
                let _ = DestroyWindow(hwnd);
                return false;
            };
            // 异步创建媒体项（fSync=FALSE 立即返回）；dwUserData 携带代次
            if player
                .CreateMediaItemFromURL(PCWSTR(url.as_ptr()), false, generation as usize, None)
                .is_err()
            {
                let _ = player.Shutdown();
                let _ = DestroyWindow(hwnd);
                return false;
            }
            ACTIVE.with(|a| *a.borrow_mut() = Some((player, hwnd.0 as isize)));
        }
        true
    }

    pub fn stop() {
        // 代次自增：在途的异步事件全部作废
        GENERATION.fetch_add(1, Ordering::SeqCst);
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

    /// 对齐子窗口到新矩形（卡片按视频宽高比自适应后调用）
    pub fn reposition(rect: (i32, i32, i32, i32)) {
        ACTIVE.with(|a| {
            if let Some((_, hwnd)) = a.borrow().as_ref() {
                unsafe {
                    let _ = MoveWindow(
                        HWND(*hwnd as *mut core::ffi::c_void),
                        rect.0,
                        rect.1,
                        rect.2,
                        rect.3,
                        true,
                    );
                }
            }
        });
    }
}

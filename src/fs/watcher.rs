//! 目录实时监听：当前目录内容被外部改动时自动刷新（无需手动 F5）。
//!
//! 设计要点：
//! - 底层用 `notify` 的推荐后端（Windows 为 ReadDirectoryChangesW）非递归监听当前目录。
//! - notify 的事件回调运行在其内部线程，不能触碰 UI；这里把「有变化」的信号通过
//!   channel 送给一个防抖线程。防抖线程在一段静默期后才回调 `on_change`，避免大量
//!   连续事件（如后台复制写入）触发过于频繁的刷新。
//! - `on_change` 由 main() 注入，内部只做 `slint::invoke_from_event_loop` 回主线程触发
//!   `auto-refresh` 回调，真正的目录重读在主线程完成。

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

/// 事件静默期：收到最后一个变化事件后再等这么久没有新事件才触发刷新。
const DEBOUNCE: Duration = Duration::from_millis(350);

/// 目录实时监听句柄。持有底层 watcher 与当前监听路径；drop 时自动停止监听。
pub struct DirWatcher {
    watcher: RecommendedWatcher,
    current: Option<PathBuf>,
    /// 持有发送端，保证防抖线程不会因 channel 断开而提前退出。
    _tx: Sender<()>,
}

impl DirWatcher {
    /// 创建监听句柄。`on_change` 在目录发生变化并经过防抖后被调用（防抖线程内）。
    pub fn new<F>(on_change: F) -> notify::Result<Self>
    where
        F: Fn() + Send + 'static,
    {
        let (tx, rx) = channel::<()>();

        // 防抖线程：阻塞等首个事件，随后在 DEBOUNCE 静默期内不断排空后续事件，
        // 静默后仅触发一次 on_change。channel 断开（DirWatcher 被 drop）时退出。
        std::thread::spawn(move || loop {
            if rx.recv().is_err() {
                break; // 发送端已释放
            }
            loop {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(_) => continue,                       // 又有新事件，继续等静默
                    Err(RecvTimeoutError::Timeout) => break, // 静默达成，触发刷新
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            on_change();
        });

        let tx_evt = tx.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if is_relevant(&ev.kind) {
                    let _ = tx_evt.send(());
                }
            }
        })?;

        Ok(Self {
            watcher,
            current: None,
            _tx: tx,
        })
    }

    /// 切换监听到指定目录（非递归）。与当前监听路径相同时不做处理。
    /// 传入非真实存在的路径会静默失败并清空当前监听。
    pub fn watch(&mut self, path: &Path) {
        if self.current.as_deref() == Some(path) {
            return;
        }
        self.clear();
        if path.is_dir()
            && self
                .watcher
                .watch(path, RecursiveMode::NonRecursive)
                .is_ok()
        {
            self.current = Some(path.to_path_buf());
        }
    }

    /// 停止当前监听（进入虚拟路径如标签/回收站时调用）。
    pub fn clear(&mut self) {
        if let Some(old) = self.current.take() {
            let _ = self.watcher.unwatch(&old);
        }
    }
}

/// 判断事件是否影响目录列表：仅忽略纯访问事件（读取/打开），其余均触发刷新。
fn is_relevant(kind: &EventKind) -> bool {
    !matches!(kind, EventKind::Access(_))
}

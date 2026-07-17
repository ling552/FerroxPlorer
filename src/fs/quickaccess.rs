//! Windows 快速访问读取与固定/取消固定
//!
//! 通过 Shell 命名空间 `shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}`（"快速访问"）枚举
//! 与资源管理器完全一致的项目（含用户固定的文件夹与常用文件夹）；
//! 通过 Shell 规范动词 `pintohome` / `unpinfromhome` 真正写入系统快速访问，
//! 因此本模块的读写都基于系统真实状态，天然「与资源管理器一致」。

/// 单个快速访问项（文件夹）
#[derive(Clone)]
pub struct QaItem {
    /// 显示名（资源管理器中看到的友好名称）
    pub name: String,
    /// 真实文件系统路径
    pub path: String,
}

// ── 列表缓存：避免每次侧边栏重建都走一次 COM 枚举 ──
#[cfg(windows)]
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
fn cache() -> &'static Mutex<Option<(Instant, Vec<QaItem>)>> {
    static C: OnceLock<Mutex<Option<(Instant, Vec<QaItem>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// 使列表缓存失效（固定/取消固定后调用，确保侧边栏立即反映变化）
#[cfg(windows)]
pub fn invalidate() {
    if let Ok(mut c) = cache().lock() {
        *c = None;
    }
}

/// 读取系统快速访问下的全部文件夹项（带 2 秒短缓存）
#[cfg(windows)]
pub fn list() -> Vec<QaItem> {
    // 命中新鲜缓存直接返回
    if let Ok(c) = cache().lock() {
        if let Some((ts, items)) = c.as_ref() {
            if ts.elapsed() < Duration::from_secs(2) {
                return items.clone();
            }
        }
    }
    let items = enumerate();
    if let Ok(mut c) = cache().lock() {
        *c = Some((Instant::now(), items.clone()));
    }
    items
}

/// 实际执行 COM 枚举
#[cfg(windows)]
fn enumerate() -> Vec<QaItem> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IEnumIDList, IShellFolder, SHGetDesktopFolder, SHParseDisplayName, SHCONTF_FOLDERS,
        SHCONTF_INCLUDEHIDDEN, SHCONTF_NONFOLDERS, SHGDN_FORPARSING, SHGDN_INFOLDER,
    };

    // 快速访问命名空间：用 shell::: 前缀让 SHParseDisplayName 解析为绝对 PIDL
    const QUICK_ACCESS: &str = "shell:::{679f85cb-0220-4080-b29b-5540cc05aab6}";

    let mut out: Vec<QaItem> = Vec::new();

    unsafe {
        // 本线程初始化 COM（STA）；已初始化返回 S_FALSE，忽略即可
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // 取桌面根 IShellFolder
        let desktop: IShellFolder = match SHGetDesktopFolder() {
            Ok(d) => d,
            Err(_) => return out,
        };

        // 解析「快速访问」的绝对 PIDL
        let wide: Vec<u16> = QUICK_ACCESS
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut qa_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut qa_pidl, 0, None).is_err()
            || qa_pidl.is_null()
        {
            return out;
        }

        // 绑定到快速访问文件夹本身
        let qa_folder: IShellFolder = match desktop.BindToObject(qa_pidl, None) {
            Ok(f) => f,
            Err(_) => {
                CoTaskMemFree(Some(qa_pidl as *const _));
                return out;
            }
        };
        CoTaskMemFree(Some(qa_pidl as *const _));

        // 枚举其中的项（文件夹 + 文件，含隐藏）；后续按真实路径过滤出文件夹
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
        let mut enum_opt: Option<IEnumIDList> = None;
        let _ = qa_folder.EnumObjects(HWND::default(), flags, &mut enum_opt);
        let Some(enum_ids) = enum_opt else {
            return out;
        };

        loop {
            let mut child: [*mut ITEMIDLIST; 1] = [std::ptr::null_mut()];
            let mut fetched: u32 = 0;
            // Next 取 1 个；返回 S_OK 且 fetched==1 才继续
            let hr = enum_ids.Next(&mut child, Some(&mut fetched));
            if hr.is_err() || fetched == 0 || child[0].is_null() {
                break;
            }
            let child_pidl = child[0];

            // 真实路径（SHGDN_FORPARSING）
            let path = display_name_of(&qa_folder, child_pidl, SHGDN_FORPARSING);
            // 友好显示名（SHGDN_INFOLDER）
            let name = display_name_of(&qa_folder, child_pidl, SHGDN_INFOLDER);

            CoTaskMemFree(Some(child_pidl as *const _));

            if let Some(path) = path {
                // 仅保留真实存在的文件夹（盘符开头、且为目录），过滤纯虚拟项与文件
                let is_dir = path.chars().nth(1).map(|c| c == ':').unwrap_or(false)
                    && std::path::Path::new(&path).is_dir();
                if is_dir {
                    let name = name.filter(|s| !s.is_empty()).unwrap_or_else(|| {
                        std::path::Path::new(&path)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone())
                    });
                    out.push(QaItem { name, path });
                }
            }
        }
    }

    // 辅助：取子项显示名并转 String
    unsafe fn display_name_of(
        folder: &windows::Win32::UI::Shell::IShellFolder,
        child: *const windows::Win32::UI::Shell::Common::ITEMIDLIST,
        flags: windows::Win32::UI::Shell::SHGDNF,
    ) -> Option<String> {
        use windows::Win32::UI::Shell::Common::STRRET;
        use windows::Win32::UI::Shell::StrRetToBufW;
        let mut strret: STRRET = STRRET::default();
        folder.GetDisplayNameOf(child, flags, &mut strret).ok()?;
        let mut buf = [0u16; 260];
        StrRetToBufW(&mut strret, Some(child), &mut buf).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        if len == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    out
}

/// 将文件夹固定到系统快速访问
#[cfg(windows)]
pub fn pin(path: &str) -> bool {
    let ok = invoke_verb(path, "pintohome");
    if ok {
        invalidate();
    }
    ok
}

/// 从系统快速访问取消固定（预留接口：供侧边栏右键「取消固定」使用）
#[cfg(windows)]
#[allow(dead_code)]
pub fn unpin(path: &str) -> bool {
    let ok = invoke_verb(path, "unpinfromhome");
    if ok {
        invalidate();
    }
    ok
}

/// 对 `path` 执行指定 Shell 规范动词（如 pintohome / unpinfromhome）
///
/// 关键：先 `QueryContextMenu` 让上下文菜单处理器完成初始化并注册动词，
/// 随后遍历菜单项，用 `GetCommandString(GCS_VERBW)` 找出规范动词名等于 `verb`
/// 的菜单项 ID，再以「命令偏移量」(MAKEINTRESOURCE) 调用 `InvokeCommand`——
/// 这是最可靠的方式（直接用动词名调用对部分处理器无效）。
#[cfg(windows)]
fn invoke_verb(path: &str, verb: &str) -> bool {
    use windows::core::{PCSTR, PCWSTR, PSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToParent, SHParseDisplayName, CMINVOKECOMMANDINFO,
        GCS_VERBW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, SW_SHOWNORMAL,
    };

    const CMF_NORMAL: u32 = 0x0000_0000;
    const ID_CMD_FIRST: u32 = 1;
    const ID_CMD_LAST: u32 = 0x7FFF;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // 路径 → 绝对 PIDL
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_err()
            || pidl.is_null()
        {
            return false;
        }

        // 绑定父文件夹 + 取子项相对 PIDL
        let mut child_pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = match SHBindToParent(pidl, Some(&mut child_pidl)) {
            Ok(p) if !child_pidl.is_null() => p,
            _ => {
                CoTaskMemFree(Some(pidl as *const _));
                return false;
            }
        };

        // 取子项 IContextMenu
        let children: [*const ITEMIDLIST; 1] = [child_pidl];
        let ctx_menu: IContextMenu = match parent.GetUIObjectOf(HWND::default(), &children, None) {
            Ok(cm) => cm,
            Err(_) => {
                CoTaskMemFree(Some(pidl as *const _));
                return false;
            }
        };

        let mut ok = false;
        if let Ok(hmenu) = CreatePopupMenu() {
            // 必须先 QueryContextMenu，处理器才会注册动词映射
            let _ = ctx_menu.QueryContextMenu(hmenu, 0, ID_CMD_FIRST, ID_CMD_LAST, CMF_NORMAL);

            // 遍历菜单项，匹配规范动词名 == verb 的菜单 ID
            let mut matched_offset: Option<u32> = None;
            let count = GetMenuItemCount(Some(hmenu));
            for i in 0..count {
                let id = GetMenuItemID(hmenu, i);
                if id == 0 || id == u32::MAX {
                    continue;
                }
                // 命令偏移量 = 菜单 ID - ID_CMD_FIRST
                let offset = id - ID_CMD_FIRST;
                let mut buf = [0u16; 128];
                if ctx_menu
                    .GetCommandString(
                        offset as usize,
                        GCS_VERBW,
                        None,
                        PSTR(buf.as_mut_ptr() as *mut u8),
                        buf.len() as u32,
                    )
                    .is_ok()
                {
                    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    let name = String::from_utf16_lossy(&buf[..len]);
                    if name.eq_ignore_ascii_case(verb) {
                        matched_offset = Some(offset);
                        break;
                    }
                }
            }

            if let Some(offset) = matched_offset {
                // 以命令偏移量调用（MAKEINTRESOURCE 约定：lpVerb 低位为命令偏移）
                let mut info = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    hwnd: HWND::default(),
                    lpVerb: PCSTR(offset as usize as *const u8),
                    nShow: SW_SHOWNORMAL.0,
                    ..Default::default()
                };
                ok = ctx_menu.InvokeCommand(&mut info).is_ok();
            }

            // 回退：直接用规范动词名调用（对部分处理器仍有效）
            if !ok {
                let verb_bytes: Vec<u8> = verb.bytes().chain(std::iter::once(0)).collect();
                let mut info = CMINVOKECOMMANDINFO {
                    cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                    hwnd: HWND::default(),
                    lpVerb: PCSTR(verb_bytes.as_ptr()),
                    nShow: SW_SHOWNORMAL.0,
                    ..Default::default()
                };
                ok = ctx_menu.InvokeCommand(&mut info).is_ok();
            }

            let _ = DestroyMenu(hmenu);
        }

        CoTaskMemFree(Some(pidl as *const _));
        ok
    }
}

// ── 非 Windows 平台空实现 ──
#[cfg(not(windows))]
pub fn list() -> Vec<QaItem> {
    Vec::new()
}
#[cfg(not(windows))]
pub fn invalidate() {}
#[cfg(not(windows))]
pub fn pin(_path: &str) -> bool {
    false
}
#[cfg(not(windows))]
#[allow(dead_code)]
pub fn unpin(_path: &str) -> bool {
    false
}

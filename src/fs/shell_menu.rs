//! Windows 原生 Shell 右键菜单
//!
//! 通过 Shell 的 `IContextMenu` 取得某个文件/文件夹的系统上下文菜单（包含
//! 资源管理器里看到的全部项目：打开方式、发送到、以及第三方应用注册的项，
//! 如 VSCode、压缩软件、TortoiseGit 等），用 `TrackPopupMenuEx` 在指定屏幕
//! 坐标弹出，并把用户选择经 `InvokeCommand` 真正执行。
//!
//! 这样「我方自绘菜单 + 系统菜单入口」混合方案中，点击「更多系统选项」即可
//! 调出与资源管理器一致的完整右键能力。

/// 在屏幕坐标 (screen_x, screen_y) 弹出 `paths` 的系统右键菜单。
/// `paths` 为同一目录下的一个或多个选中项（多选时菜单作用于全部项，
/// 与资源管理器一致）；`hwnd_isize` 为宿主窗口句柄（isize 形式，便于跨 crate 传递）。
/// 返回是否真正执行了某条命令（用户选了项并 InvokeCommand），供调用方据此刷新视图。
#[cfg(windows)]
pub fn show(paths: &[String], hwnd_isize: isize, screen_x: i32, screen_y: i32) -> bool {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToParent, SHParseDisplayName, CMINVOKECOMMANDINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetMenuItemCount, TrackPopupMenuEx, SW_SHOWNORMAL,
        TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    };

    // QueryContextMenu 标志（windows 0.59 中为裸 u32 常量，这里按官方数值书写）
    const CMF_NORMAL: u32 = 0x0000_0000;
    const CMF_EXPLORE: u32 = 0x0000_0004;

    // 菜单命令 ID 范围：自建 HMENU 的命令从该值起编号
    const ID_CMD_FIRST: u32 = 1;
    const ID_CMD_LAST: u32 = 0x7FFF;

    if paths.is_empty() {
        return false;
    }

    unsafe {
        // 本线程初始化 COM（菜单需 STA 公寓）；已初始化返回 S_FALSE，忽略即可
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let hwnd = HWND(hwnd_isize as *mut core::ffi::c_void);

        // 逐个把路径解析为绝对 PIDL；解析失败的项跳过（如虚拟路径）
        let mut abs_pidls: Vec<*mut ITEMIDLIST> = Vec::with_capacity(paths.len());
        for p in paths {
            let wide: Vec<u16> = p.encode_utf16().chain(std::iter::once(0)).collect();
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_ok()
                && !pidl.is_null()
            {
                abs_pidls.push(pidl);
            }
        }

        // 统一释放所有绝对 PIDL 的辅助闭包
        let free_all = |list: &[*mut ITEMIDLIST]| {
            for &p in list {
                CoTaskMemFree(Some(p as *const _));
            }
        };

        if abs_pidls.is_empty() {
            return false;
        }

        // 以首项绑定到父文件夹 IShellFolder（多选项默认同属一个目录），并取得其子项相对 PIDL
        let mut first_child: *mut ITEMIDLIST = std::ptr::null_mut();
        let parent: IShellFolder = match SHBindToParent(abs_pidls[0], Some(&mut first_child)) {
            Ok(p) if !first_child.is_null() => p,
            _ => {
                free_all(&abs_pidls);
                return false;
            }
        };

        // 收集全部子项相对 PIDL：首项已得；其余项用 SHBindToParent 取相对 PIDL。
        // 注意 SHBindToParent 返回的相对 PIDL 指向其内部，不可单独释放（随绝对 PIDL 释放）。
        let mut children: Vec<*const ITEMIDLIST> = vec![first_child as *const ITEMIDLIST];
        for &abs in abs_pidls.iter().skip(1) {
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            // 仅需取相对 PIDL，不使用返回的父文件夹，故显式标注泛型类型
            if SHBindToParent::<IShellFolder>(abs, Some(&mut child)).is_ok() && !child.is_null() {
                children.push(child as *const ITEMIDLIST);
            }
        }

        // 从父文件夹取得（多）子项的 IContextMenu
        let ctx_menu: IContextMenu = match parent.GetUIObjectOf(hwnd, &children, None) {
            Ok(cm) => cm,
            Err(_) => {
                free_all(&abs_pidls);
                return false;
            }
        };

        // 创建空弹出菜单，让系统把项填进去
        let Ok(hmenu) = CreatePopupMenu() else {
            free_all(&abs_pidls);
            return false;
        };

        // QueryContextMenu 返回 HRESULT；失败位（高位）则放弃
        let hr = ctx_menu.QueryContextMenu(
            hmenu,
            0,
            ID_CMD_FIRST,
            ID_CMD_LAST,
            CMF_NORMAL | CMF_EXPLORE,
        );
        if hr.is_err() || GetMenuItemCount(Some(hmenu)) <= 0 {
            let _ = DestroyMenu(hmenu);
            free_all(&abs_pidls);
            return false;
        }

        // 弹出菜单，TPM_RETURNCMD 让函数返回所选命令 ID 而不直接派发
        let cmd = TrackPopupMenuEx(
            hmenu,
            (TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
            screen_x,
            screen_y,
            hwnd,
            None,
        );

        // 用户选择了某项：经 InvokeCommand 执行（命令 ID 需减去起始偏移）
        let mut invoked = false;
        if cmd.0 != 0 {
            let verb_id = (cmd.0 as u32) - ID_CMD_FIRST;
            let mut info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd,
                // lpVerb 用 MAKEINTRESOURCE 约定：低位字为命令序号
                lpVerb: PCSTR(verb_id as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = ctx_menu.InvokeCommand(&mut info);
            invoked = true;
        }

        let _ = DestroyMenu(hmenu);
        free_all(&abs_pidls);
        invoked
    }
}

/// 非 Windows 平台空实现。
#[cfg(not(windows))]
pub fn show(_paths: &[String], _hwnd_isize: isize, _screen_x: i32, _screen_y: i32) -> bool {
    false
}

/// 在屏幕坐标弹出目录「背景」右键菜单（资源管理器空白处菜单：查看/排序/新建/粘贴等）。
/// `dir` 为当前浏览目录；返回是否执行了某条命令（供调用方刷新视图）。
#[cfg(windows)]
pub fn show_background(dir: &str, hwnd_isize: isize, screen_x: i32, screen_y: i32) -> bool {
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        IContextMenu, IShellFolder, SHBindToObject, SHParseDisplayName, CMINVOKECOMMANDINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetMenuItemCount, TrackPopupMenuEx, SW_SHOWNORMAL,
        TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    };

    const CMF_NORMAL: u32 = 0x0000_0000;
    const ID_CMD_FIRST: u32 = 1;
    const ID_CMD_LAST: u32 = 0x7FFF;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hwnd = HWND(hwnd_isize as *mut core::ffi::c_void);

        // 目录路径 → 绝对 PIDL → 绑定该目录自身的 IShellFolder
        let wide: Vec<u16> = dir.encode_utf16().chain(std::iter::once(0)).collect();
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_err()
            || pidl.is_null()
        {
            return false;
        }
        let folder: Result<IShellFolder, _> = SHBindToObject(None, pidl, None);
        let folder = match folder {
            Ok(f) => f,
            Err(_) => {
                CoTaskMemFree(Some(pidl as *const _));
                return false;
            }
        };

        // 目录视图背景菜单对象（与资源管理器空白处右键一致）
        let ctx_menu: IContextMenu = match folder.CreateViewObject(hwnd) {
            Ok(cm) => cm,
            Err(_) => {
                CoTaskMemFree(Some(pidl as *const _));
                return false;
            }
        };

        let Ok(hmenu) = CreatePopupMenu() else {
            CoTaskMemFree(Some(pidl as *const _));
            return false;
        };
        let hr = ctx_menu.QueryContextMenu(hmenu, 0, ID_CMD_FIRST, ID_CMD_LAST, CMF_NORMAL);
        if hr.is_err() || GetMenuItemCount(Some(hmenu)) <= 0 {
            let _ = DestroyMenu(hmenu);
            CoTaskMemFree(Some(pidl as *const _));
            return false;
        }

        let cmd = TrackPopupMenuEx(
            hmenu,
            (TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
            screen_x,
            screen_y,
            hwnd,
            None,
        );

        let mut invoked = false;
        if cmd.0 != 0 {
            let verb_id = (cmd.0 as u32) - ID_CMD_FIRST;
            let mut info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd,
                lpVerb: PCSTR(verb_id as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = ctx_menu.InvokeCommand(&mut info);
            invoked = true;
        }

        let _ = DestroyMenu(hmenu);
        CoTaskMemFree(Some(pidl as *const _));
        invoked
    }
}

#[cfg(not(windows))]
pub fn show_background(_dir: &str, _hwnd_isize: isize, _screen_x: i32, _screen_y: i32) -> bool {
    false
}

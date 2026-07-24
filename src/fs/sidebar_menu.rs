//! 侧栏导航项专用右键菜单。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarCommand {
    Open,
    OpenNewTab,
    OpenNewPanel,
    Unpin,
}

#[cfg(windows)]
pub fn show(
    hwnd_isize: isize,
    screen_x: i32,
    screen_y: i32,
    can_open_extra: bool,
    can_unpin: bool,
) -> Option<SidebarCommand> {
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, TrackPopupMenuEx, MF_SEPARATOR, MF_STRING,
        TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON,
    };

    const OPEN: usize = 1;
    const NEW_TAB: usize = 2;
    const NEW_PANEL: usize = 3;
    const UNPIN: usize = 4;

    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return None;
        };
        let _ = AppendMenuW(menu, MF_STRING, OPEN, w!("打开"));
        if can_open_extra {
            let _ = AppendMenuW(menu, MF_STRING, NEW_TAB, w!("在新标签页中打开"));
            let _ = AppendMenuW(menu, MF_STRING, NEW_PANEL, w!("在新面板中打开"));
        }
        if can_unpin {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, UNPIN, w!("从快速访问取消固定"));
        }
        let hwnd = HWND(hwnd_isize as *mut core::ffi::c_void);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD).0,
            screen_x,
            screen_y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            OPEN => Some(SidebarCommand::Open),
            NEW_TAB => Some(SidebarCommand::OpenNewTab),
            NEW_PANEL => Some(SidebarCommand::OpenNewPanel),
            UNPIN => Some(SidebarCommand::Unpin),
            _ => None,
        }
    }
}

#[cfg(not(windows))]
pub fn show(
    _hwnd_isize: isize,
    _screen_x: i32,
    _screen_y: i32,
    _can_open_panel: bool,
    _can_unpin: bool,
) -> Option<SidebarCommand> {
    None
}

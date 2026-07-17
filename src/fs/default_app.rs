//! 「设为默认文件管理器」：通过 HKCU 注册表接管文件夹/驱动器的打开动词与
//! 「此电脑」新窗口（Win+E / 任务栏资源管理器图标）。
//!
//! 全部写入 HKEY_CURRENT_USER（无需管理员权限）。HKCR 是 HKCU 与 HKLM 的合并视图
//! 且 HKCU 优先，因此：
//! - 开启：在 HKCU\Software\Classes 写 Directory/Drive 的 shell\open\command 指向本
//!   应用（%1 传目录路径），并显式写空的 DelegateExecute 覆盖 HKLM 侧的资源管理器
//!   委托（值级合并，不覆盖则系统仍走 DelegateExecute COM）；
//!   另接管 This PC CLSID 的 opennewwindow 动词使 Win+E 打开本应用。
//! - 关闭：删除上述 HKCU 键。这些键在 HKCU 默认不存在，删除即回落到 HKLM 提供的
//!   系统默认（资源管理器），无需备份还原。

/// This PC（此电脑）的 CLSID：其 opennewwindow 动词对应 Win+E 与任务栏图标
#[cfg(windows)]
const THIS_PC_KEY: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow";

/// This PC 的 open 动词：部分入口（桌面「此电脑」双击、开始菜单固定的资源管理器项等）
/// 走 open 而非 opennewwindow，一并接管确保「点击资源管理器」也打开本应用
#[cfg(windows)]
const THIS_PC_OPEN_KEY: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open";

/// 接管的文件系统类：目录与驱动器根
#[cfg(windows)]
const FS_CLASSES: [&str; 2] = ["Directory", "Drive"];

/// 开启/关闭「默认文件管理器」。返回 Err 时注册表操作失败（UI 提示用户）。
/// 开启中途失败会尽力回滚已写入的键，避免「开关显示关闭但部分类已被接管」的残留态。
pub fn set_default(enable: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if enable {
            match enable_win() {
                Ok(()) => Ok(()),
                Err(e) => {
                    let _ = disable_win(); // 清理半成品接管（对不存在的键幂等）
                    Err(e)
                }
            }
        } else {
            disable_win()
        }
    }
    #[cfg(not(windows))]
    {
        let _ = enable;
        Ok(())
    }
}

#[cfg(windows)]
fn enable_win() -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let exe = std::env::current_exe()?.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);

    // 双击文件夹 / 盘符 → 本应用（%1 为目标路径）
    for cls in FS_CLASSES {
        let (cmd, _) =
            hkcu.create_subkey(format!(r"Software\Classes\{}\shell\open\command", cls))?;
        cmd.set_value("", &format!("\"{}\" \"%1\"", exe))?;
        // 覆盖 HKLM 侧的 DelegateExecute（空值 = 不委托，直接执行 command）
        cmd.set_value("DelegateExecute", &"")?;
    }

    // Win+E / 任务栏「资源管理器」→ 本应用（无目标路径，进入默认起始页）
    let (cmd, _) = hkcu.create_subkey(format!(r"{}\command", THIS_PC_KEY))?;
    cmd.set_value("", &format!("\"{}\"", exe))?;
    cmd.set_value("DelegateExecute", &"")?;
    // 桌面「此电脑」双击 / 其它走 open 动词的资源管理器入口 → 本应用
    let (cmd, _) = hkcu.create_subkey(format!(r"{}\command", THIS_PC_OPEN_KEY))?;
    cmd.set_value("", &format!("\"{}\"", exe))?;
    cmd.set_value("DelegateExecute", &"")?;
    Ok(())
}

#[cfg(windows)]
fn disable_win() -> std::io::Result<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    // 「键不存在」视为已恢复；其余错误（如 ACL 被第三方锁定）必须上报，
    // 否则 UI 会误报「已恢复」而接管实际残留
    fn del(hkcu: &RegKey, path: String) -> std::io::Result<()> {
        match hkcu.delete_subkey_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for cls in FS_CLASSES {
        // 删除整个 open 动词键（HKCU 下由本应用创建）
        del(&hkcu, format!(r"Software\Classes\{}\shell\open", cls))?;
    }
    del(&hkcu, THIS_PC_KEY.to_string())?;
    del(&hkcu, THIS_PC_OPEN_KEY.to_string())?;
    Ok(())
}

/// 查询当前是否处于「已接管」状态（按 Directory 的 command 是否指向本应用判断）。
/// 用于启动时校准配置与注册表的实际状态（如 exe 被移动后开关仍显示开启）。
pub fn is_default() -> bool {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(cmd) = hkcu.open_subkey(r"Software\Classes\Directory\shell\open\command") else {
            return false;
        };
        let val: String = cmd.get_value("").unwrap_or_default();
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        !exe.is_empty() && val.contains(&exe)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

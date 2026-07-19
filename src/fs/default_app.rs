//! 「设为默认文件管理器」：通过 HKCU 注册表接管文件夹、驱动器与「此电脑」入口。
//!
//! 接管前会在本应用专属注册表键中备份原 command 值，并在写入的 command 键上留下
//! 所有权标记。关闭开关或卸载时只恢复仍由 FerroxPlorer 持有的项；若用户后来改用
//! 其它文件管理器，则不会删除或覆盖对方的注册表设置。

#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
const THIS_PC_KEY: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow";
#[cfg(windows)]
const THIS_PC_OPEN_KEY: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open";
#[cfg(windows)]
const BACKUP_ROOT: &str = r"Software\FerroxPlorer\DefaultFileManager";
#[cfg(windows)]
const OWNER_VALUE: &str = "FerroxPlorerOwner";

#[cfg(windows)]
struct Target {
    id: &'static str,
    verb_key: &'static str,
    with_target: bool,
}

#[cfg(windows)]
const TARGETS: [Target; 4] = [
    Target {
        id: "Directory",
        verb_key: r"Software\Classes\Directory\shell\open",
        with_target: true,
    },
    Target {
        id: "Drive",
        verb_key: r"Software\Classes\Drive\shell\open",
        with_target: true,
    },
    Target {
        id: "ThisPcOpen",
        verb_key: THIS_PC_OPEN_KEY,
        with_target: false,
    },
    Target {
        id: "ThisPcOpenNewWindow",
        verb_key: THIS_PC_KEY,
        with_target: false,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationState {
    Disabled,
    Complete,
    Repairable,
    External,
}

/// 开启/关闭默认文件管理器。关闭时仅恢复仍由本应用持有的注册项。
pub fn set_default(enable: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let result = if enable { enable_win() } else { disable_win() };
        // 注册表操作不是事务；即使中途失败，也通知 Shell 重新读取实际状态。
        notify_shell_associations_changed();
        result
    }
    #[cfg(not(windows))]
    {
        let _ = enable;
        Ok(())
    }
}

/// 检查完整状态。只有带本应用所有权记录的缺失项或旧 FerroxPlorer 路径可自动修复；
/// 若任一入口已指向其它程序，返回 External，调用方不得静默夺回关联。
pub fn registration_state() -> RegistrationState {
    #[cfg(windows)]
    {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let active = hkcu
            .open_subkey(BACKUP_ROOT)
            .ok()
            .and_then(|key| key.get_value::<u32, _>("Active").ok())
            == Some(1);
        if !active {
            // v0.2.0 及更早版本没有备份/所有权标记；四项都仍指向 FerroxPlorer 时，
            // 允许一次性迁移到新模型。原始 HKCU 状态已不可知，迁移时按系统默认回落处理。
            return if legacy_registration(&hkcu) {
                RegistrationState::Repairable
            } else {
                RegistrationState::Disabled
            };
        }

        let exe = match std::env::current_exe() {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(_) => return RegistrationState::Repairable,
        };
        let mut needs_repair = false;
        for target in &TARGETS {
            let expected = expected_command(&exe, target.with_target);
            let command_path = format!(r"{}\command", target.verb_key);
            let Ok(command) = hkcu.open_subkey(&command_path) else {
                needs_repair = true;
                continue;
            };
            let actual: String = command.get_value("").unwrap_or_default();
            let owner = command.get_value::<u32, _>(OWNER_VALUE).unwrap_or(0) == 1;
            let delegate: Option<String> = command.get_value("DelegateExecute").ok();
            if owner && command_matches(&actual, &expected) && delegate.as_deref() == Some("") {
                continue;
            }
            if owner && command_targets_ferroxplorer(&actual) {
                needs_repair = true;
                continue;
            }
            return RegistrationState::External;
        }
        if needs_repair {
            RegistrationState::Repairable
        } else {
            RegistrationState::Complete
        }
    }
    #[cfg(not(windows))]
    {
        RegistrationState::Disabled
    }
}

#[cfg(windows)]
fn expected_command(exe: &str, with_target: bool) -> String {
    if with_target {
        format!("\"{}\" \"%1\"", exe)
    } else {
        format!("\"{}\"", exe)
    }
}

#[cfg(windows)]
fn command_matches(actual: &str, expected: &str) -> bool {
    actual.trim().eq_ignore_ascii_case(expected)
}

#[cfg(windows)]
fn command_targets_ferroxplorer(command: &str) -> bool {
    command
        .trim()
        .to_ascii_lowercase()
        .contains("ferroxplorer.exe")
}

#[cfg(windows)]
fn legacy_registration(hkcu: &RegKey) -> bool {
    TARGETS.iter().all(|target| {
        hkcu.open_subkey(format!(r"{}\command", target.verb_key))
            .ok()
            .and_then(|command| command.get_value::<String, _>("").ok())
            .is_some_and(|command| command_targets_ferroxplorer(&command))
    })
}

#[cfg(windows)]
fn backup_target(
    hkcu: &RegKey,
    backup_root: &RegKey,
    target: &Target,
    legacy_migration: bool,
) -> std::io::Result<()> {
    let backup = backup_root.create_subkey(target.id)?.0;
    let verb_existed = !legacy_migration && hkcu.open_subkey(target.verb_key).is_ok();
    let command_path = format!(r"{}\command", target.verb_key);
    let command = if legacy_migration {
        None
    } else {
        hkcu.open_subkey(&command_path).ok()
    };
    backup.set_value("VerbExisted", &(verb_existed as u32))?;
    backup.set_value("CommandExisted", &(command.is_some() as u32))?;

    if let Some(command) = command {
        match command.get_raw_value("") {
            Ok(value) => {
                backup.set_value("DefaultExisted", &1u32)?;
                backup.set_raw_value("DefaultValue", &value)?;
            }
            Err(_) => backup.set_value("DefaultExisted", &0u32)?,
        }
        match command.get_raw_value("DelegateExecute") {
            Ok(value) => {
                backup.set_value("DelegateExisted", &1u32)?;
                backup.set_raw_value("DelegateValue", &value)?;
            }
            Err(_) => backup.set_value("DelegateExisted", &0u32)?,
        }
    } else {
        backup.set_value("DefaultExisted", &0u32)?;
        backup.set_value("DelegateExisted", &0u32)?;
    }
    Ok(())
}

#[cfg(windows)]
fn enable_win() -> std::io::Result<()> {
    let exe = std::env::current_exe()?.to_string_lossy().to_string();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let already_active = hkcu
        .open_subkey(BACKUP_ROOT)
        .ok()
        .and_then(|key| key.get_value::<u32, _>("Active").ok())
        == Some(1);

    if !already_active {
        // 旧版四项都指向 FerroxPlorer 时，按“此前没有用户自定义 HKCU 动词”迁移；
        // 新的用户操作则完整备份当前值。
        let legacy_migration = legacy_registration(&hkcu);
        let _ = hkcu.delete_subkey_all(BACKUP_ROOT);
        let backup_root = hkcu.create_subkey(BACKUP_ROOT)?.0;
        for target in &TARGETS {
            backup_target(&hkcu, &backup_root, target, legacy_migration)?;
        }
        backup_root.set_value("Active", &1u32)?;
    }

    for target in &TARGETS {
        let command_path = format!(r"{}\command", target.verb_key);
        let (command, _) = hkcu.create_subkey(command_path)?;
        command.set_value("", &expected_command(&exe, target.with_target))?;
        command.set_value("DelegateExecute", &"")?;
        command.set_value(OWNER_VALUE, &1u32)?;
    }
    Ok(())
}

#[cfg(windows)]
fn restore_raw_value(
    command: &RegKey,
    backup: &RegKey,
    existed_name: &str,
    backup_name: &str,
    target_name: &str,
) -> std::io::Result<()> {
    if backup.get_value::<u32, _>(existed_name).unwrap_or(0) == 1 {
        let value = backup.get_raw_value(backup_name)?;
        command.set_raw_value(target_name, &value)
    } else {
        match command.delete_value(target_name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[cfg(windows)]
fn delete_empty_key(parent: &RegKey, child: &str) -> std::io::Result<()> {
    match parent.delete_subkey(child) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(145) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn disable_win() -> std::io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let backup_root = match hkcu.open_subkey(BACKUP_ROOT) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut first_error = None;

    for target in &TARGETS {
        let command_path = format!(r"{}\command", target.verb_key);
        let Ok(command) = hkcu.open_subkey_with_flags(
            &command_path,
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        ) else {
            continue;
        };
        let actual: String = command.get_value("").unwrap_or_default();
        let owner = command.get_value::<u32, _>(OWNER_VALUE).unwrap_or(0) == 1;
        if !owner || !command_targets_ferroxplorer(&actual) {
            continue;
        }

        let result = (|| -> std::io::Result<()> {
            let backup = backup_root.open_subkey(target.id)?;
            restore_raw_value(&command, &backup, "DefaultExisted", "DefaultValue", "")?;
            restore_raw_value(
                &command,
                &backup,
                "DelegateExisted",
                "DelegateValue",
                "DelegateExecute",
            )?;
            let _ = command.delete_value(OWNER_VALUE);
            drop(command);

            if backup.get_value::<u32, _>("CommandExisted").unwrap_or(0) == 0 {
                if let Some((parent, child)) = command_path.rsplit_once('\\') {
                    if let Ok(parent) = hkcu.open_subkey_with_flags(
                        parent,
                        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
                    ) {
                        delete_empty_key(&parent, child)?;
                    }
                }
            }
            if backup.get_value::<u32, _>("VerbExisted").unwrap_or(0) == 0 {
                if let Some((parent, child)) = target.verb_key.rsplit_once('\\') {
                    if let Ok(parent) = hkcu.open_subkey_with_flags(
                        parent,
                        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
                    ) {
                        delete_empty_key(&parent, child)?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }

    if first_error.is_none() {
        let _ = hkcu.delete_subkey_all(BACKUP_ROOT);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(windows)]
fn notify_shell_associations_changed() {
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::{command_matches, command_targets_ferroxplorer, expected_command};

    #[test]
    fn directory_command_requires_exact_path_and_target() {
        let expected = expected_command(r"C:\Apps\FerroxPlorer\ferroxplorer.exe", true);
        assert!(command_matches(
            r#""C:\Apps\FerroxPlorer\ferroxplorer.exe" "%1""#,
            &expected
        ));
        assert!(!command_matches(
            r#""C:\Apps\FerroxPlorer\ferroxplorer.exe""#,
            &expected
        ));
        assert!(!command_matches(
            r#""C:\Apps\FerroxPlorer\ferroxplorer.exe.old" "%1""#,
            &expected
        ));
    }

    #[test]
    fn ownership_check_rejects_other_file_managers() {
        assert!(command_targets_ferroxplorer(
            r#""D:\Old\FerroxPlorer.exe" "%1""#
        ));
        assert!(!command_targets_ferroxplorer(
            r#""C:\Tools\OtherExplorer.exe" "%1""#
        ));
    }
}

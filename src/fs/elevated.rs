//! 受保护位置的文件操作按需提权。

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const ARG: &str = "--elevated-file-op";

#[derive(Clone, Copy)]
pub enum ElevatedOp {
    CreateDir,
    CreateFile,
    Rename,
    Recycle,
}

impl ElevatedOp {
    fn name(self) -> &'static str {
        match self {
            Self::CreateDir => "mkdir",
            Self::CreateFile => "touch",
            Self::Rename => "rename",
            Self::Recycle => "recycle",
        }
    }

    fn parse(value: &OsStr) -> Option<Self> {
        match value.to_str()? {
            "mkdir" => Some(Self::CreateDir),
            "touch" => Some(Self::CreateFile),
            "rename" => Some(Self::Rename),
            "recycle" => Some(Self::Recycle),
            _ => None,
        }
    }
}

/// 若当前进程是提权文件操作子进程，则执行操作并返回 true。
pub fn handle_startup_args() -> bool {
    let args: Vec<OsString> = std::env::args_os().collect();
    let Some(pos) = args.iter().position(|arg| arg == ARG) else {
        return false;
    };
    let Some(op) = args.get(pos + 1).and_then(|value| ElevatedOp::parse(value)) else {
        return true;
    };
    let values = &args[pos + 2..];
    let result = match op {
        ElevatedOp::CreateDir if values.len() == 2 => values[1]
            .to_str()
            .ok_or_else(invalid_name)
            .and_then(|name| {
                crate::fs::operations::new_folder(Path::new(&values[0]), name).map(|_| ())
            }),
        ElevatedOp::CreateFile if values.len() == 2 => values[1]
            .to_str()
            .ok_or_else(invalid_name)
            .and_then(|name| {
                crate::fs::operations::new_file(Path::new(&values[0]), name).map(|_| ())
            }),
        ElevatedOp::Rename if values.len() == 2 => values[1]
            .to_str()
            .ok_or_else(invalid_name)
            .and_then(|name| {
                crate::fs::operations::rename(Path::new(&values[0]), name).map(|_| ())
            }),
        ElevatedOp::Recycle if !values.is_empty() => {
            let paths: Vec<PathBuf> = values.iter().map(PathBuf::from).collect();
            crate::fs::recyclebin::move_to_recycle_bin(&paths)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "无效的提权文件操作参数",
        )),
    };
    if let Err(error) = result {
        eprintln!("提权文件操作失败：{error}");
    }
    true
}

fn invalid_name() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, "文件名不是有效 Unicode")
}

/// 仅在权限被拒绝时请求 UAC；返回是否成功启动提权子进程。
pub fn retry_if_permission_denied(
    error: &std::io::Error,
    op: ElevatedOp,
    args: &[OsString],
) -> bool {
    if error.kind() != std::io::ErrorKind::PermissionDenied && error.raw_os_error() != Some(5) {
        return false;
    }
    run_as_admin(op, args)
}

#[cfg(windows)]
fn run_as_admin(op: ElevatedOp, args: &[OsString]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn quote(value: &OsStr) -> Vec<u16> {
        let mut result = vec![b'"' as u16];
        let mut slashes = 0usize;
        for ch in value.encode_wide() {
            if ch == b'\\' as u16 {
                slashes += 1;
            } else if ch == b'"' as u16 {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
                result.push(ch);
                slashes = 0;
            } else {
                result.extend(std::iter::repeat_n(b'\\' as u16, slashes));
                slashes = 0;
                result.push(ch);
            }
        }
        result.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        result.push(b'"' as u16);
        result
    }

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut params: Vec<u16> = ARG.encode_utf16().collect();
    for value in std::iter::once(OsString::from(op.name())).chain(args.iter().cloned()) {
        params.push(b' ' as u16);
        params.extend(quote(&value));
    }
    params.push(0);
    let exe_w: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let runas_w: Vec<u16> = "runas".encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            runas_w.as_ptr(),
            exe_w.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}

#[cfg(not(windows))]
fn run_as_admin(_op: ElevatedOp, _args: &[OsString]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_permission_errors_are_not_elevated() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        assert!(!retry_if_permission_denied(
            &error,
            ElevatedOp::CreateFile,
            &[]
        ));
    }

    #[test]
    fn elevated_operations_are_whitelisted() {
        assert!(ElevatedOp::parse(OsStr::new("mkdir")).is_some());
        assert!(ElevatedOp::parse(OsStr::new("powershell")).is_none());
    }
}

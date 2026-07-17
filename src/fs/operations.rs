//! 文件系统操作：目录读取、复制、移动、删除、重命名、新建

use super::metadata::{classify, unix_ts, Entry};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// 读取目录内容，返回条目列表（文件夹优先，再按名称排序）
/// `show_hidden` 为 false 时过滤掉以点开头或带 Windows 隐藏属性的项目。
pub fn read_dir(path: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in fs::read_dir(path)? {
        let dirent = match dirent {
            Ok(d) => d,
            Err(_) => continue,
        };
        let p = dirent.path();
        let meta = match dirent.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        let name = dirent.file_name().to_string_lossy().to_string();
        if !show_hidden && is_hidden(&name, &meta) {
            continue;
        }
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let (icon_class, icon_label, kind) = classify(&p, is_dir);

        entries.push(Entry {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir,
            size_bytes: if is_dir { 0 } else { meta.len() },
            modified_ts: unix_ts(modified),
            kind,
            icon_label,
            icon_class,
        });
    }
    Ok(entries)
}

/// 判断条目是否为隐藏项：名称以点开头，或带 Windows FILE_ATTRIBUTE_HIDDEN(0x2) 属性。
fn is_hidden(name: &str, meta: &fs::Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = meta;
    false
}

/// 重命名
pub fn rename(old: &Path, new_name: &str) -> io::Result<PathBuf> {
    let parent = old.parent().unwrap_or(Path::new("."));
    let new_path = parent.join(new_name);
    fs::rename(old, &new_path)?;
    Ok(new_path)
}

/// 永久删除（不经回收站）。回收站清空等不可逆场景使用；
/// 普通删除请走 `recyclebin::move_to_recycle_bin`。
#[allow(dead_code)]
pub fn delete(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// 递归复制目录或文件，自动处理同名冲突（追加 副本）。
/// 同步实现，UI 粘贴路径现走 `tasks` 异步队列；此处保留供测试与同步调用。
#[allow(dead_code)]
pub fn copy_into(src: &Path, dst_dir: &Path) -> io::Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无效源路径"))?;
    let mut target = dst_dir.join(file_name);
    target = resolve_conflict(target);

    if src.is_dir() {
        copy_dir_recursive(src, &target)?;
    } else {
        fs::copy(src, &target)?;
    }
    Ok(target)
}

/// 移动（同盘 rename，跨盘 复制后删除）。
/// 同步实现，UI 粘贴路径现走 `tasks` 异步队列；此处保留供测试与同步调用。
#[allow(dead_code)]
pub fn move_into(src: &Path, dst_dir: &Path) -> io::Result<PathBuf> {
    let file_name = src
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无效源路径"))?;
    let mut target = dst_dir.join(file_name);
    target = resolve_conflict(target);

    match fs::rename(src, &target) {
        Ok(_) => Ok(target),
        Err(_) => {
            // 跨盘：复制后删除源
            if src.is_dir() {
                copy_dir_recursive(src, &target)?;
                fs::remove_dir_all(src)?;
            } else {
                fs::copy(src, &target)?;
                fs::remove_file(src)?;
            }
            Ok(target)
        }
    }
}

/// 解决同名冲突：name -> name (2) -> name (3)
pub(crate) fn resolve_conflict(mut target: PathBuf) -> PathBuf {
    if !target.exists() {
        return target;
    }
    let parent = target.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = target.extension().map(|e| e.to_string_lossy().to_string());

    let mut n = 2;
    loop {
        let candidate_name = match &ext {
            Some(e) => format!("{} ({}).{}", stem, n, e),
            None => format!("{} ({})", stem, n),
        };
        target = parent.join(candidate_name);
        if !target.exists() {
            return target;
        }
        n += 1;
        if n > 9999 {
            return target;
        }
    }
}

#[allow(dead_code)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 新建文件夹，自动避免重名
pub fn new_folder(parent: &Path, base: &str) -> io::Result<PathBuf> {
    let target = resolve_conflict(parent.join(base));
    fs::create_dir(&target)?;
    Ok(target)
}

/// 新建空文件，自动避免重名
pub fn new_file(parent: &Path, base: &str) -> io::Result<PathBuf> {
    let target = resolve_conflict(parent.join(base));
    fs::File::create(&target)?;
    Ok(target)
}

/// 支持的归档格式
#[derive(Clone, Copy, PartialEq)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
}

/// 判断路径的归档格式（按扩展名）。非归档返回 None。
pub fn is_archive(path: &Path) -> Option<ArchiveFormat> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "zip" => Some(ArchiveFormat::Zip),
        "7z" => Some(ArchiveFormat::SevenZ),
        "tar" => Some(ArchiveFormat::Tar),
        "gz" | "tgz" => Some(ArchiveFormat::TarGz),
        _ => None,
    }
}

/// 判断路径是否为可解压归档（任意支持格式）。保留旧名以兼容 UI 调用。
pub fn is_zip_archive(path: &Path) -> bool {
    is_archive(path).is_some()
}

/// 归档基名：单项用其文件名（去扩展名），多项用「首项 等」。
/// 压缩任务入队时用它与 `resolve_conflict` 确定归档输出路径。
pub fn archive_stem(items: &[PathBuf]) -> String {
    let first_name = items[0]
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "归档".to_string());
    if items.len() == 1 {
        items[0]
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(first_name)
    } else {
        format!("{} 等", first_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    // 创建隔离的临时测试目录
    fn temp_dir() -> PathBuf {
        let mut d = env::temp_dir();
        let unique = format!(
            "ferrox_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        d.push(unique);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_new_folder_and_conflict() {
        let dir = temp_dir();
        let a = new_folder(&dir, "测试").unwrap();
        assert!(a.is_dir());
        // 同名再建应得到 "测试 (2)"
        let b = new_folder(&dir, "测试").unwrap();
        assert!(b.is_dir());
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_string_lossy().contains("(2)"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_new_file_and_rename() {
        let dir = temp_dir();
        let f = new_file(&dir, "笔记.txt").unwrap();
        assert!(f.is_file());
        let renamed = rename(&f, "新笔记.txt").unwrap();
        assert!(renamed.is_file());
        assert!(!f.exists());
        assert_eq!(renamed.file_name().unwrap().to_string_lossy(), "新笔记.txt");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_copy_and_move() {
        let dir = temp_dir();
        let src_dir = dir.join("源");
        let dst_dir = dir.join("目标");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        let file = src_dir.join("数据.bin");
        fs::write(&file, b"hello").unwrap();

        // 复制：源仍在，目标出现
        let copied = copy_into(&file, &dst_dir).unwrap();
        assert!(file.exists());
        assert!(copied.exists());
        assert_eq!(fs::read(&copied).unwrap(), b"hello");

        // 移动：源消失
        let moved = move_into(&file, &dst_dir).unwrap();
        assert!(!file.exists());
        assert!(moved.exists());

        fs::remove_dir_all(&dir).ok();
    }

    // 归档压缩/解压的往返测试迁至 fs::tasks（覆盖真实的后台流式实现）

    #[test]
    fn test_read_dir_classify() {
        let dir = temp_dir();
        fs::write(dir.join("a.rs"), b"fn main(){}").unwrap();
        fs::create_dir(dir.join("子目录")).unwrap();
        let entries = read_dir(&dir, true).unwrap();
        assert_eq!(entries.len(), 2);
        let rs = entries.iter().find(|e| e.name == "a.rs").unwrap();
        assert_eq!(rs.icon_class, "code");
        assert_eq!(rs.icon_label, "RS");
        let sub = entries.iter().find(|e| e.name == "子目录").unwrap();
        assert!(sub.is_dir);
        fs::remove_dir_all(&dir).ok();
    }
}

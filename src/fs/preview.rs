//! 空格键 Quick Look 预览内容计算（类 macOS「快速查看」）
//!
//! 仅负责把"选中文件 / 文件夹"归类并产出可直接显示的文本/统计信息；
//! 图片的大图渲染复用 thumbnail 模块，由 ui_bridge 在更大尺寸下提取位图。

use std::path::Path;

/// 预览归类：决定 Quick Look 浮层用哪种方式展示
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewKind {
    /// 图片：渲染缩略图大图
    Image,
    /// 文本/代码：显示文件首部内容
    Text,
    /// 文件夹：统计顶层项数与大小
    Folder,
    /// 视频：内嵌播放（Media Foundation 子窗口渲染，含音频）
    Video,
    /// 其它：仅展示图标与基础信息
    Info,
}

impl PreviewKind {
    /// 传给 Slint 的整型编码（与 quick_look.slint 约定一致）
    /// 0 信息 / 1 图片 / 2 文本 / 3 文件夹 / 4 视频
    pub fn code(self) -> i32 {
        match self {
            PreviewKind::Info => 0,
            PreviewKind::Image => 1,
            PreviewKind::Text => 2,
            PreviewKind::Folder => 3,
            PreviewKind::Video => 4,
        }
    }
}

/// 可作为图片大图预览的扩展名（与缩略图提取能力一致）
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico",
];

/// 可内嵌播放的视频扩展名（Media Foundation 支持的常见容器）
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "wmv", "m4v", "webm", "mpg", "mpeg",
];

/// 可作为纯文本预览的扩展名（含常见源码 / 配置 / 文档）
const TEXT_EXTS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "log",
    "ini",
    "cfg",
    "conf",
    "toml",
    "yaml",
    "yml",
    "json",
    "xml",
    "csv",
    "rs",
    "go",
    "py",
    "js",
    "ts",
    "jsx",
    "tsx",
    "c",
    "h",
    "cpp",
    "hpp",
    "cc",
    "cs",
    "java",
    "kt",
    "rb",
    "php",
    "sh",
    "bat",
    "ps1",
    "css",
    "scss",
    "less",
    "html",
    "htm",
    "slint",
    "sql",
    "lua",
    "vue",
    "svelte",
    "gradle",
    "properties",
    "env",
    "gitignore",
    "dockerfile",
    "makefile",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// 判断给定路径的预览类型
pub fn kind_of(path: &Path, is_dir: bool) -> PreviewKind {
    if is_dir {
        return PreviewKind::Folder;
    }
    let ext = ext_of(path);
    if IMAGE_EXTS.contains(&ext.as_str()) {
        PreviewKind::Image
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        PreviewKind::Video
    } else if TEXT_EXTS.contains(&ext.as_str()) {
        PreviewKind::Text
    } else {
        PreviewKind::Info
    }
}

/// 读取文本文件首部，最多 `max_bytes` 字节并按 UTF-8 有损转换。
/// 截断时在结尾追加省略提示。读取失败返回错误说明文本。
pub fn read_text_head(path: &Path, max_bytes: usize) -> String {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return format!("无法读取文件：{}", e),
    };
    let mut buf = vec![0u8; max_bytes];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return format!("读取出错：{}", e),
    };
    buf.truncate(n);
    // 检测是否为二进制（含 NUL 字节）：避免把二进制文件当文本显示成乱码
    if buf.iter().any(|&b| b == 0) {
        return "（二进制内容，无法以文本预览）".to_string();
    }
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    // 文件比读取窗口更大时提示已截断
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() as usize > n {
            text.push_str("\n\n…（仅显示开头部分）");
        }
    }
    text
}

/// 文件夹顶层统计：返回 (子文件夹数, 文件数, 顶层文件总字节)。
/// 仅统计直接子项，不递归，避免大目录卡顿。
pub fn folder_summary(path: &Path) -> (usize, usize, u64) {
    let mut dirs = 0usize;
    let mut files = 0usize;
    let mut size = 0u64;
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => dirs += 1,
                Ok(_) => {
                    files += 1;
                    if let Ok(m) = entry.metadata() {
                        size += m.len();
                    }
                }
                Err(_) => {}
            }
        }
    }
    (dirs, files, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kind_of() {
        assert_eq!(kind_of(Path::new("a.png"), false), PreviewKind::Image);
        assert_eq!(kind_of(Path::new("a.rs"), false), PreviewKind::Text);
        assert_eq!(kind_of(Path::new("a.bin"), false), PreviewKind::Info);
        assert_eq!(kind_of(Path::new("anything"), true), PreviewKind::Folder);
        // 大小写不敏感
        assert_eq!(kind_of(Path::new("A.PNG"), false), PreviewKind::Image);
    }

    #[test]
    fn test_read_text_head() {
        let mut p = std::env::temp_dir();
        p.push(format!("ferrox_prev_{}.txt", std::process::id()));
        std::fs::write(&p, b"hello world").unwrap();
        let t = read_text_head(&p, 1024);
        assert!(t.contains("hello world"));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_read_text_head_truncate() {
        let mut p = std::env::temp_dir();
        p.push(format!("ferrox_prev_big_{}.txt", std::process::id()));
        std::fs::write(&p, vec![b'x'; 5000]).unwrap();
        let t = read_text_head(&p, 100);
        assert!(t.contains("仅显示开头部分"));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn test_binary_detect() {
        let mut p = std::env::temp_dir();
        p.push(format!("ferrox_prev_bin_{}.dat", std::process::id()));
        std::fs::write(&p, [0u8, 1, 2, 3, 0, 5]).unwrap();
        let t = read_text_head(&p, 1024);
        assert!(t.contains("二进制"));
        std::fs::remove_file(&p).ok();
    }
}

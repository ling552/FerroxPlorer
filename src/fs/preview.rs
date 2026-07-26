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
    /// 归档：列出压缩包内文件（内容区复用文本面板显示清单）
    Archive,
    /// 其它：仅展示图标与基础信息
    Info,
}

impl PreviewKind {
    /// 传给 Slint 的整型编码（与 quick_look.slint 约定一致）
    /// 0 信息 / 1 图片 / 2 文本 / 3 文件夹 / 4 视频。
    /// 归档清单以等宽文本展示，直接复用文本面板（编码 2）。
    pub fn code(self) -> i32 {
        match self {
            PreviewKind::Info => 0,
            PreviewKind::Image => 1,
            PreviewKind::Text | PreviewKind::Archive => 2,
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
/// 注意：kind_of 已对非图片/视频/归档/二进制文件统一兜底为文本预览，本表仅供 renderable_web
/// 之外的特殊判断参考，当前无直接引用，保留以备未来按扩展名区分高亮等用途。
#[allow(dead_code)]
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "log", "ini", "cfg", "conf", "toml", "yaml", "yml", "json", "xml",
    "csv", "rs", "go", "py", "js", "ts", "jsx", "tsx", "c", "h", "cpp", "hpp", "cc", "cs", "java",
    "kt", "rb", "php", "sh", "bat", "ps1", "css", "scss", "less", "html", "htm", "slint", "sql",
    "lua", "vue", "svelte", "gradle", "properties", "env", "gitignore", "dockerfile", "makefile",
    "gitattributes", "dockerignore", "npmignore", "editorconfig", "license", "readme", "lock",
];

/// 二进制/可执行文件扩展名：预览时显示应用基本信息而非文本内容
const BINARY_EXTS: &[&str] = &[
    "exe", "msi", "dll", "sys", "com", "scr",
    "iso", "bin", "dat", "img", "vhd", "vhdx",
    "cab", "msu", "dmp", "pdb",
    "deb", "rpm", "appimage",
    "dmg", "pkg",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// 是否支持「渲染视图」（WebView2 显示网页效果，与源码视图可切换）：
/// Markdown 转 HTML 渲染；HTML/HTM 直接渲染；PHP 渲染其中的静态 HTML 部分
pub fn renderable_web(path: &Path) -> bool {
    matches!(
        ext_of(path).as_str(),
        "md" | "markdown" | "html" | "htm" | "php"
    )
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
    } else if is_archive_kind(&ext, path) {
        PreviewKind::Archive
    } else if is_binary_kind(&ext) {
        // EXE/MSI/ISO 等二进制文件：显示应用基本信息而非文本内容
        PreviewKind::Info
    } else {
        // 兜底用文本预览：文本文件显示内容，二进制文件由 read_text_head 的
        // NUL 检测给出「二进制内容，无法以文本预览」提示（用户要求大部分文件以文本打开）
        PreviewKind::Text
    }
}

/// 是否作为二进制/可执行文件（预览时显示信息而非文本）
fn is_binary_kind(ext: &str) -> bool {
    BINARY_EXTS.contains(&ext)
}

/// 是否作为归档预览（与 operations::is_archive 一致的格式集合）
fn is_archive_kind(ext: &str, _path: &Path) -> bool {
    matches!(
        ext,
        "zip" | "7z" | "tar" | "gz" | "tgz"
    )
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

/// 列出归档内文件清单（供空格预览展示）。按格式读取条目名/大小/是否目录，
/// 上限 2000 项防超大归档卡顿；失败返回错误说明文本。复用 tasks.rs 已验证的
/// 读取路径（zip::ZipArchive / sevenz_rust::SevenZReader / tar::Archive）。
pub fn archive_listing(path: &Path) -> String {
    use std::io::Read;
    let ext = ext_of(path);
    // 用闭包包裹使 `?` 可用（外层函数返回 String，不能直接用 `?`）
    let result: Result<Vec<(String, u64, bool)>, String> = (|| {
        let mut items: Vec<(String, u64, bool)> = Vec::new();
        match ext.as_str() {
            "zip" => {
                let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mut zip = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
                for i in 0..zip.len() {
                    if items.len() >= 2000 {
                        break;
                    }
                    if let Ok(e) = zip.by_index_raw(i) {
                        items.push((e.name().to_string(), e.size(), e.is_dir()));
                    }
                }
            }
            "7z" => {
                let sz = sevenz_rust::SevenZReader::open(path, sevenz_rust::Password::empty())
                    .map_err(|e| e.to_string())?;
                for e in sz.archive().files.iter() {
                    if items.len() >= 2000 {
                        break;
                    }
                    items.push((e.name().to_string(), e.size, e.is_directory()));
                }
            }
            "tar" => {
                let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let mut tar = tar::Archive::new(f);
                for entry in tar.entries().map_err(|e| e.to_string())? {
                    if items.len() >= 2000 {
                        break;
                    }
                    if let Ok(e) = entry {
                        let name = e.path()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let size = e.header().size().unwrap_or(0);
                        let is_dir = e.header().entry_type().is_dir();
                        items.push((name, size, is_dir));
                    }
                }
            }
            "tgz" | "gz" => {
                // tar.gz / tgz 需 gzip 解码；纯 .gz（非 tar 流）退回单文件条目
                let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
                let gz = flate2::read::GzDecoder::new(f);
                let mut tar = tar::Archive::new(gz);
                let mut pulled = 0;
                for entry in tar.entries().map_err(|e| e.to_string())? {
                    if items.len() >= 2000 {
                        break;
                    }
                    if let Ok(e) = entry {
                        let name = e.path()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let size = e.header().size().unwrap_or(0);
                        let is_dir = e.header().entry_type().is_dir();
                        items.push((name, size, is_dir));
                        pulled += 1;
                    }
                }
                if pulled == 0 {
                    // 普通 .gz 单文件：用解压后的原始文件名与解压大小
                    let mut f2 = std::fs::File::open(path).map_err(|e| e.to_string())?;
                    let mut dec = flate2::read::GzDecoder::new(&mut f2);
                    let mut buf = Vec::new();
                    let size = dec.read_to_end(&mut buf).unwrap_or(0) as u64;
                    let stem = path
                        .file_name()
                        .map(|n| n.to_string_lossy().trim_end_matches(".gz").to_string())
                        .unwrap_or_else(|| "解压内容".into());
                    items.push((stem, size, false));
                }
            }
            _ => return Err("不支持的归档格式".into()),
        }
        Ok(items)
    })();
    match result {
        Ok(items) => format_listing(&items),
        Err(e) => format!("无法读取归档：{}", e),
    }
}

/// 把 (名, 大小, 是否目录) 列表格式化为预览文本
fn format_listing(items: &[(String, u64, bool)]) -> String {
    if items.is_empty() {
        return "（归档为空）".to_string();
    }
    use std::fmt::Write;
    let mut out = String::new();
    let dirs = items.iter().filter(|(_, _, d)| *d).count();
    let files = items.len() - dirs;
    let total: u64 = items.iter().map(|(_, s, _)| s).sum();
    let _ = writeln!(out, "📦 归档内容 | 共 {} 项", items.len());
    let _ = writeln!(out, "   ├─ 📁 {} 个文件夹", dirs);
    let _ = writeln!(out, "   ├─ 📄 {} 个文件", files);
    let _ = writeln!(out, "   └─ 💾 合计 {}\n", super::metadata::human_size(total));
    let _ = writeln!(out, "{}", "─".repeat(60));
    for (name, size, is_dir) in items {
        let _ = writeln!(
            out,
            "{}  {}",
            if *is_dir { "📁" } else { "📄" },
            if *is_dir {
                name.clone()
            } else {
                format!("{}  ({})", name, super::metadata::human_size(*size))
            }
        );
    }
    out
}

/// 文件夹顶层统计：返回 (子文件夹数, 文件数, 顶层文件总字节)。
/// 仅统计直接子项，不递归，避免大目录卡顿。
pub fn folder_summary(path: &Path) -> (usize, usize, u64) {    let mut dirs = 0usize;
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
        // 非图片/视频/归档文件统一兜底为文本预览（二进制内容由 read_text_head 检测提示）
        assert_eq!(kind_of(Path::new("a.bin"), false), PreviewKind::Text);
        assert_eq!(kind_of(Path::new("a.zip"), false), PreviewKind::Archive);
        assert_eq!(kind_of(Path::new("a.7z"), false), PreviewKind::Archive);
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

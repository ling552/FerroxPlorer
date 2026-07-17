//! 文件名索引：后台扫描磁盘建立「路径 + 名称」索引，加速深层（含子文件夹）搜索。
//!
//! - 重建在后台线程执行，进度经回调上报（fraction 按顶层子目录完成数估算，
//!   与 UI 进度条对应；条数与当前目录用于副标题文案）。
//! - 索引持久化到 %APPDATA%/FerroxPlorer/index.txt，启动后首次搜索时惰性加载。
//! - 搜索：给定目录前缀 + 关键字，返回构造好的 Entry 列表（上限截断）。

use super::metadata::{classify, unix_ts, Entry};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 索引单条记录
struct IndexEntry {
    path: String,       // 完整路径
    name_lower: String, // 文件名小写（大小写不敏感匹配用）
    is_dir: bool,
}

/// 整份索引（Arc 共享，重建完成后整体替换）
struct IndexData {
    entries: Vec<IndexEntry>,
}

/// 内存中的当前索引；None 表示尚未加载（惰性从磁盘读）
fn index_slot() -> &'static Mutex<Option<Arc<IndexData>>> {
    static S: OnceLock<Mutex<Option<Arc<IndexData>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// 是否正在重建（防止并发重建）
static REBUILDING: AtomicBool = AtomicBool::new(false);

pub fn is_rebuilding() -> bool {
    REBUILDING.load(Ordering::Relaxed)
}

/// 索引文件路径（与 config.toml 同目录）
fn index_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("FerroxPlorer").join("index.txt"))
}

/// 磁盘上是否已有索引文件
pub fn exists() -> bool {
    index_file().map(|p| p.is_file()).unwrap_or(false)
}

/// 空闲时展示的索引概况文案；无索引返回空串
pub fn summary() -> String {
    match load() {
        Some(data) => format!("索引就绪：共 {} 项，深层搜索可用", data.entries.len()),
        None => String::new(),
    }
}

/// 取当前索引：内存命中直接返回，否则从磁盘惰性加载
fn load() -> Option<Arc<IndexData>> {
    if let Some(cur) = index_slot().lock().ok()?.clone() {
        return Some(cur);
    }
    let path = index_file()?;
    let file = std::fs::File::open(path).ok()?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        // 行格式："D\t<路径>" / "F\t<路径>"
        let Some((kind, p)) = line.split_once('\t') else {
            continue;
        };
        let name_lower = Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        entries.push(IndexEntry {
            path: p.to_string(),
            name_lower,
            is_dir: kind == "D",
        });
    }
    let data = Arc::new(IndexData { entries });
    *index_slot().lock().ok()? = Some(data.clone());
    Some(data)
}

/// 索引范围 → 扫描根目录集合。user=用户目录（custom 暂同 user），all=全部固定磁盘。
fn roots_for(scope: &str) -> Vec<PathBuf> {
    match scope {
        "all" => super::disk::list_disks()
            .into_iter()
            .filter(|d| matches!(d.kind, super::disk::DriveKind::Fixed))
            .map(|d| PathBuf::from(d.root))
            .collect(),
        _ => dirs::home_dir().into_iter().collect(),
    }
}

/// 应跳过的目录（系统/回收站等，扫描无意义且量大）
fn skip_dir(name: &str) -> bool {
    name.starts_with('$')
        || name.eq_ignore_ascii_case("System Volume Information")
        || name.eq_ignore_ascii_case("WinSxS")
}

/// 重建索引（在后台线程调用，阻塞直到完成）。
/// `progress(fraction, 已索引条数, 当前目录显示名)` 至多约每 150ms 回调一次。
/// 返回索引条目总数。
pub fn rebuild(scope: &str, progress: impl Fn(f32, u64, String)) -> Result<u64, String> {
    if REBUILDING.swap(true, Ordering::SeqCst) {
        return Err("已有重建任务在进行".into());
    }
    // 离开函数时清除标志（含错误路径）
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            REBUILDING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = Guard;

    let roots = roots_for(scope);
    if roots.is_empty() {
        return Err("没有可索引的目录".into());
    }

    // 进度单元 = 各根目录的顶层子目录（根自身的散文件算一个单元），
    // fraction = 已完成单元 / 总单元，可给出真实推进的进度条
    let mut units: Vec<PathBuf> = Vec::new();
    for root in &roots {
        units.push(root.clone()); // 根目录散文件单元（不递归）
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if skip_dir(&name) {
                    continue;
                }
                let ft = match e.file_type() {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if ft.is_dir() && !ft.is_symlink() {
                    units.push(e.path());
                }
            }
        }
    }
    let total_units = units.len().max(1);

    let mut entries: Vec<IndexEntry> = Vec::new();
    let mut last_report = Instant::now();
    let mut report = |done_units: usize, count: u64, current: &Path, force: bool| {
        if force || last_report.elapsed() >= Duration::from_millis(150) {
            let frac = (done_units as f32 / total_units as f32).clamp(0.0, 1.0);
            let disp = current.to_string_lossy().to_string();
            progress(frac, count, disp);
            last_report = Instant::now();
        }
    };

    for (ui_idx, unit) in units.iter().enumerate() {
        let root_unit = roots.iter().any(|r| r == unit);
        // 单元自身也入索引（根目录除外）
        if !root_unit {
            entries.push(IndexEntry {
                path: unit.to_string_lossy().to_string(),
                name_lower: unit
                    .file_name()
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
                is_dir: true,
            });
        }
        // 迭代式深度优先（避免深层目录递归栈溢出）
        let mut stack: Vec<PathBuf> = vec![unit.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if skip_dir(&name) {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                let p = e.path();
                let is_dir = ft.is_dir();
                entries.push(IndexEntry {
                    path: p.to_string_lossy().to_string(),
                    name_lower: name.to_lowercase(),
                    is_dir,
                });
                // 根目录散文件单元不递归（子目录单元会覆盖）；
                // 符号链接/junction 不下钻，避免环
                if is_dir && !ft.is_symlink() && !root_unit {
                    stack.push(p);
                }
            }
            report(ui_idx, entries.len() as u64, &dir, false);
        }
        report(ui_idx + 1, entries.len() as u64, unit, false);
    }
    report(total_units, entries.len() as u64, Path::new(""), true);

    // 落盘（原子性从简：先写临时文件再改名）
    let path = index_file().ok_or("无法定位配置目录")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("txt.tmp");
    {
        let file = std::fs::File::create(&tmp).map_err(|e| format!("写入索引失败：{e}"))?;
        let mut wtr = BufWriter::new(file);
        for e in &entries {
            let _ = writeln!(wtr, "{}\t{}", if e.is_dir { "D" } else { "F" }, e.path);
        }
        wtr.flush().map_err(|e| format!("写入索引失败：{e}"))?;
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("写入索引失败：{e}"))?;

    let count = entries.len() as u64;
    // 整体替换内存索引
    if let Ok(mut slot) = index_slot().lock() {
        *slot = Some(Arc::new(IndexData { entries }));
    }
    Ok(count)
}

/// 深层搜索：在 `dir` 目录（含全部子目录）范围内查找名称含 `query` 的条目。
/// 未建立索引返回 None（调用方回退到当前目录过滤）；结果按 `limit` 截断。
pub fn search(dir: &Path, query: &str, case_sensitive: bool, limit: usize) -> Option<Vec<Entry>> {
    let data = load()?;
    // 目录前缀（统一补路径分隔符，避免 "C:\ab" 命中 "C:\abc\..."）
    let mut prefix = dir.to_string_lossy().to_string();
    if !prefix.ends_with('\\') && !prefix.ends_with('/') {
        prefix.push('\\');
    }
    let prefix_lower = prefix.to_lowercase();
    let query_lower = query.to_lowercase();

    let mut results = Vec::new();
    for e in &data.entries {
        if results.len() >= limit {
            break;
        }
        // 名称匹配
        let hit = if case_sensitive {
            // 大小写敏感：对原始文件名匹配
            Path::new(&e.path)
                .file_name()
                .map(|n| n.to_string_lossy().contains(query))
                .unwrap_or(false)
        } else {
            e.name_lower.contains(&query_lower)
        };
        if !hit {
            continue;
        }
        // 目录范围匹配（Windows 路径大小写不敏感）
        if e.path.len() <= prefix.len() {
            continue;
        }
        if !e.path[..].to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        // 构造 Entry（补充真实元数据；已删除的失效索引项跳过）
        let path = Path::new(&e.path);
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| e.path.clone());
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let (icon_class, icon_label, kind) = classify(path, e.is_dir);
        results.push(Entry {
            name,
            path: e.path.clone(),
            is_dir: e.is_dir,
            size_bytes: if e.is_dir { 0 } else { meta.len() },
            modified_ts: unix_ts(modified),
            kind,
            icon_label,
            icon_class,
        });
    }
    Some(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 跳过系统目录() {
        assert!(skip_dir("$Recycle.Bin"));
        assert!(skip_dir("System Volume Information"));
        assert!(skip_dir("WinSxS"));
        assert!(!skip_dir("Documents"));
    }
}

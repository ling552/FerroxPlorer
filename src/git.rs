//! Git 集成：读取当前目录所属仓库的分支名与工作区文件状态（只读）。
//!
//! 用 libgit2（git2 crate）向上发现仓库，对工作区计算一次 status，生成
//! 「文件绝对路径 → 状态字符」映射。状态字符与 UI 徽章约定一致：
//! `M` 已修改 / `?` 未追踪 / `A` 已暂存 / `!` 冲突。

use std::collections::HashMap;
use std::path::Path;

/// 当前目录的 Git 信息
pub struct GitInfo {
    /// 当前分支短名（如 `main`）；分离 HEAD 时回退为 `HEAD`
    pub branch: String,
    /// 文件绝对路径（统一反斜杠）→ 状态字符，仅含有变更的文件
    statuses: HashMap<String, char>,
}

/// 读取 `dir` 所属 Git 仓库的分支与工作区状态。非仓库或读取失败返回 None。
pub fn status_for_dir(dir: &Path) -> Option<GitInfo> {
    let repo = git2::Repository::discover(dir).ok()?;
    let workdir = repo.workdir()?.to_path_buf();

    // 当前分支短名；分离 HEAD（无 shorthand）时回退 "HEAD"
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| "HEAD".to_string());

    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(false)
        // 未追踪目录不逐层展开（其父目录会被聚合标记，避免海量条目拖慢）
        .recurse_untracked_dirs(false);

    let statuses = repo.statuses(Some(&mut opts)).ok()?;
    let mut map = HashMap::new();
    for entry in statuses.iter() {
        if let Some(rel) = entry.path() {
            let ch = status_char(entry.status());
            if ch != ' ' {
                let full = workdir.join(rel);
                map.insert(normalize(&full.to_string_lossy()), ch);
            }
        }
    }
    Some(GitInfo {
        branch,
        statuses: map,
    })
}

impl GitInfo {
    /// 查询某路径的状态字符串（供 UI 徽章）。
    /// 文件：直接查表；目录：其下含任意变更项时聚合为 `M`。无状态返回空串。
    pub fn status_of(&self, path: &str, is_dir: bool) -> String {
        let norm = normalize(path);
        if let Some(&c) = self.statuses.get(&norm) {
            return c.to_string();
        }
        if is_dir {
            let prefix = if norm.ends_with('\\') {
                norm
            } else {
                format!("{}\\", norm)
            };
            if self.statuses.keys().any(|k| k.starts_with(&prefix)) {
                return "M".to_string();
            }
        }
        String::new()
    }
}

/// 路径规范化：统一为反斜杠，便于与 read_dir 得到的 Windows 风格路径比较。
fn normalize(path: &str) -> String {
    path.replace('/', "\\")
}

/// 把 git2 状态位映射为单字符徽章标识
/// （优先级：冲突 `!` > 未追踪 `?` > 已暂存 `A` > 已修改 `M`）。
fn status_char(s: git2::Status) -> char {
    if s.contains(git2::Status::CONFLICTED) {
        '!'
    } else if s.contains(git2::Status::WT_NEW) {
        '?'
    } else if s.intersects(
        git2::Status::INDEX_NEW
            | git2::Status::INDEX_MODIFIED
            | git2::Status::INDEX_DELETED
            | git2::Status::INDEX_RENAMED
            | git2::Status::INDEX_TYPECHANGE,
    ) {
        'A'
    } else if s.intersects(
        git2::Status::WT_MODIFIED
            | git2::Status::WT_DELETED
            | git2::Status::WT_RENAMED
            | git2::Status::WT_TYPECHANGE,
    ) {
        'M'
    } else {
        ' '
    }
}

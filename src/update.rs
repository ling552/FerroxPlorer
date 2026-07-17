//! 应用内更新：通过 GitHub Releases 检查新版本、带进度下载安装程序、启动安装。
//!
//! 设计要点：网络请求全部在后台线程执行（ureq 为阻塞式 API），进度通过
//! 回调闭包上报，由调用方（main.rs）在闭包内用 `slint::invoke_from_event_loop`
//! 回到主线程刷新 UI —— 与 `fs::tasks` 后台任务同一模式。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// GitHub 仓库标识（owner/name），检查更新与「关于」页链接共用
pub const REPO: &str = "ling552/FerroxPlorer";
/// 仓库主页地址（「关于」页展示 + 浏览器打开）
pub const REPO_URL: &str = "https://github.com/ling552/FerroxPlorer";
/// 当前应用版本（编译时取自 Cargo.toml）
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 最新 Release 的关键信息
#[derive(Clone)]
pub struct ReleaseInfo {
    /// 版本号（已去掉 tag 的 "v" 前缀），如 "0.2.0"
    pub version: String,
    /// 更新说明（Release 正文，可能为空）
    pub notes: String,
    /// 安装程序资产的直链下载地址
    pub asset_url: String,
    /// 安装程序文件名，如 "FerroxPlorer-Setup-0.2.0.exe"
    pub asset_name: String,
    /// 安装程序大小（字节；API 提供，用于进度分母）
    pub asset_size: u64,
}

/// 解析 "x.y.z" 版本号为数字元组（缺段按 0，非数字段截断）
fn parse_ver(v: &str) -> (u64, u64, u64) {
    let mut it = v
        .trim()
        .trim_start_matches(['v', 'V'])
        .split('.')
        .map(|s| {
            s.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
                .unwrap_or(0)
        });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// 语义化比较：latest 是否比 current 更新
pub fn is_newer(latest: &str, current: &str) -> bool {
    parse_ver(latest) > parse_ver(current)
}

/// 请求 GitHub API 获取最新 Release。
/// 返回 Err(用户可读的中文错误信息)。在后台线程调用（阻塞最长 ~15s）。
pub fn check_latest() -> Result<ReleaseInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = ureq::get(&url)
        // GitHub API 要求 User-Agent，否则 403
        .set("User-Agent", "FerroxPlorer-Updater")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| match e {
            // 404：仓库尚无任何 Release
            ureq::Error::Status(404, _) => "仓库暂无发布版本".to_string(),
            ureq::Error::Status(code, _) => format!("GitHub 接口返回 {code}"),
            ureq::Error::Transport(t) => format!("网络请求失败：{t}"),
        })?;

    let json: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("解析响应失败：{e}"))?;

    let tag = json["tag_name"].as_str().unwrap_or_default();
    if tag.is_empty() {
        return Err("响应中缺少版本号".to_string());
    }
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    let notes = json["body"].as_str().unwrap_or_default().trim().to_string();

    // 在资产列表中定位 Windows 安装程序（.exe；兼容打包成 zip 的情形）
    let assets = json["assets"].as_array().cloned().unwrap_or_default();
    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.to_ascii_lowercase().ends_with(".exe"))
                .unwrap_or(false)
        })
        .or_else(|| assets.first())
        .ok_or_else(|| "该版本未附带安装程序".to_string())?;

    Ok(ReleaseInfo {
        version,
        notes,
        asset_url: asset["browser_download_url"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        asset_name: asset["name"].as_str().unwrap_or("update.exe").to_string(),
        asset_size: asset["size"].as_u64().unwrap_or(0),
    })
}

/// 流式下载安装程序到系统临时目录，返回落盘路径。
///
/// `progress(已下载字节, 总字节, 瞬时速度 B/s)` 约每 150ms 回调一次
/// （另在开始与结束各回调一次）；`cancel` 置位后尽快中断并清理半成品。
/// 在后台线程调用。
pub fn download(
    info: &ReleaseInfo,
    cancel: &Arc<AtomicBool>,
    progress: impl Fn(u64, u64, f64),
) -> Result<PathBuf, String> {
    let resp = ureq::get(&info.asset_url)
        .set("User-Agent", "FerroxPlorer-Updater")
        .timeout(Duration::from_secs(3600))
        .call()
        .map_err(|e| format!("下载请求失败：{e}"))?;

    // 优先信任响应头的 Content-Length，缺失时退回 API 报告的资产大小
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(info.asset_size);

    let dest = std::env::temp_dir().join(&info.asset_name);
    let mut file =
        File::create(&dest).map_err(|e| format!("创建临时文件失败：{e}"))?;

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    // 速度按上报间隔内的增量计算，平滑且无需滑动窗口
    let mut last_report = Instant::now();
    let mut last_bytes: u64 = 0;
    progress(0, total, 0.0);

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&dest);
            return Err("已取消".to_string());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&dest);
                return Err(format!("下载中断：{e}"));
            }
        };
        file.write_all(&buf[..n])
            .map_err(|e| format!("写入失败：{e}"))?;
        downloaded += n as u64;

        let dt = last_report.elapsed();
        if dt >= Duration::from_millis(150) {
            let speed = (downloaded - last_bytes) as f64 / dt.as_secs_f64();
            progress(downloaded, total.max(downloaded), speed);
            last_report = Instant::now();
            last_bytes = downloaded;
        }
    }
    file.flush().map_err(|e| format!("写入失败：{e}"))?;
    progress(downloaded, downloaded.max(total), 0.0);
    Ok(dest)
}

/// 启动已下载的安装程序（分离进程）。成功后调用方应退出应用以便覆盖安装。
pub fn run_installer(path: &Path) -> Result<(), String> {
    std::process::Command::new(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动安装程序失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 版本号解析() {
        assert_eq!(parse_ver("0.1.0"), (0, 1, 0));
        assert_eq!(parse_ver("v1.2.3"), (1, 2, 3));
        assert_eq!(parse_ver("V2.0"), (2, 0, 0));
        assert_eq!(parse_ver("1.2.3-beta"), (1, 2, 3));
        assert_eq!(parse_ver(""), (0, 0, 0));
    }

    #[test]
    fn 版本比较() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
    }
}

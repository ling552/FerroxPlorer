//! Rust ↔ Slint 桥接：把 AppCore 状态推送到 UI 模型

use crate::app::{AppCore, TabKind};
use crate::fs::{disk, metadata};
use crate::{
    AclAce, AppState, CertInfo, Crumb, FileEntry, MainWindow, MetaRow, NavItem, NetAccount, TabInfo,
};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 缩略图加载代数：每次重建 entries 模型自增，后台线程据此丢弃过期结果，
/// 避免快速切换目录时旧目录的缩略图错填到新目录的行上。
static THUMB_GEN: AtomicU64 = AtomicU64::new(0);

/// 右侧面板独立的缩略图代数（与左侧互不干扰）
static R_THUMB_GEN: AtomicU64 = AtomicU64::new(0);

/// 缩略图回填目标面板：左侧主视图 entries / 右侧双面板 r_entries
#[derive(Clone, Copy, PartialEq)]
enum ThumbSide {
    Left,
    Right,
}

impl ThumbSide {
    fn generation(&self) -> &'static AtomicU64 {
        match self {
            ThumbSide::Left => &THUMB_GEN,
            ThumbSide::Right => &R_THUMB_GEN,
        }
    }
}

/// 缩略图请求的方形边长。网格视图 58px 在 icon-scale=2.0 下达 116px，128 已勉强够用，
/// 但很多真实缩略图/图标源分辨率本就充裕，提到 256（对齐 SHIL_JUMBO 上限与开发文档
/// "128px/256px" 规格）能让放大后仍保持清晰，只有系统本身只注册 32/48px 图标的极少数
/// 文件类型仍会因源分辨率不足而模糊——这是 Windows Shell 自身限制，应用层无法解决。
const THUMB_SIZE: u32 = 256;

/// 判断扩展名是否为图片或视频（内置图标模式下仍为这两类显示真实缩略图）
fn is_media(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        // 图片
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "tif" | "tiff"
        // 视频
        | "mp4" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "m4v" | "webm" | "mpg" | "mpeg"
    )
}

/// 根据设置与条目来源生成明确的图标请求。
/// `device://` 只使用类型/设备请求，不能进入真实路径 Shell 提取器。
fn icon_request_for_entry(
    e: &metadata::Entry,
    system_icons: bool,
) -> Option<crate::fs::thumbnail::IconRequest> {
    use crate::fs::thumbnail::IconRequest;

    if e.path.starts_with("device://") {
        if !system_icons {
            return None;
        }
        if e.icon_class == "device" {
            return Some(IconRequest::Device);
        }
        let extension = Path::new(&e.name)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("")
            .to_string();
        return Some(IconRequest::Type {
            extension,
            is_dir: e.is_dir,
        });
    }
    if crate::fs::virtualfs::is_virtual(&e.path) {
        return None;
    }
    if system_icons || (!e.is_dir && is_media(&e.path)) {
        Some(IconRequest::RealPath {
            path: e.path.clone(),
            is_dir: e.is_dir,
            mtime: e.modified_ts,
        })
    } else {
        None
    }
}

/// 由缓存的图标像素构建 Slint 图像（必须在 UI 线程调用）。
fn image_from(ic: &crate::fs::thumbnail::IconPixels) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(ic.w, ic.h);
    buf.make_mut_bytes().copy_from_slice(&ic.pixels);
    Image::from_rgba8(buf)
}

/// 为侧边栏条目提取系统图标（仅在 icon-source=系统图标 时调用）。
/// 在 UI 线程同步执行：文件夹图标走类型缓存（首次约 50ms，后续命中缓存为 0ms），
/// 驱动器盘符走路径缓存（首次每个约 100ms）。侧栏条目少（通常 <20），短暂阻塞可接受。
/// 虚拟路径（recycle:// / network:// / tag://）不取系统图标，返回 has-thumb=false。
fn sidebar_icon(path: &str, is_dir: bool) -> (Image, bool) {
    if crate::fs::virtualfs::is_virtual(path) {
        return (Image::default(), false);
    }
    match crate::fs::thumbnail::load_cached(path, is_dir, 0, THUMB_SIZE) {
        Some(ic) => (image_from(&ic), true),
        None => (Image::default(), false),
    }
}

/// 为"此电脑"平铺视图构建驱动器容量字段：(已用比例, 副标题, 容量条颜色)。
/// 非"此电脑"视图或非驱动器条目返回 (0.0, 空, 透明)，平铺视图据此不绘制容量条。
fn disk_fields(
    e: &metadata::Entry,
    this_pc: bool,
    disks: &[disk::DiskInfo],
) -> (f32, SharedString, slint::Brush) {
    let transparent = slint::Brush::SolidColor(slint::Color::from_argb_u8(0, 0, 0, 0));
    if !this_pc || e.icon_class != "drive" {
        return (0.0, SharedString::new(), transparent);
    }
    match disks.iter().find(|d| d.root == e.path) {
        Some(d) => {
            let ratio = d.used_ratio();
            let info = format!(
                "可用 {} / 共 {}",
                metadata::human_size(d.free),
                metadata::human_size(d.total)
            );
            // 接近写满（>=90%）用红色提示，否则用主题蓝
            let color = if ratio >= 0.9 {
                slint::Color::from_rgb_u8(0xe5, 0x39, 0x35)
            } else {
                slint::Color::from_rgb_u8(0x00, 0x78, 0xd4)
            };
            (ratio, info.into(), slint::Brush::SolidColor(color))
        }
        None => (0.0, SharedString::new(), transparent),
    }
}

/// 异步图标加载任务：(行下标, 明确的图标请求)
type IconJob = (usize, crate::fs::thumbnail::IconRequest);

/// 把当前目录条目推送到 UI（entries / crumbs / 标题 / 状态栏）
pub fn push_entries(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let tab = core.active_tab();

    // 图标来源："system" 全部文件取系统图标/缩略图；"builtin" 仅图片/视频取真实缩略图，其余用矢量图
    let icon_source = core.config.settings.icon_source.clone();
    let system_icons = icon_source == "system";

    // "此电脑"平铺视图：预取一次驱动器容量信息，用于绘制每个盘符的容量条
    let cur_is_this_pc =
        tab.history.current().to_string_lossy() == crate::fs::virtualfs::THIS_PC_PATH;
    let disks: Vec<disk::DiskInfo> = if cur_is_this_pc {
        disk::list_disks()
    } else {
        Vec::new()
    };

    // 先同步查图标缓存：命中的条目首帧即显示系统图标（无闪烁），未命中的留待异步加载。
    let icon_requests: Vec<Option<crate::fs::thumbnail::IconRequest>> = tab
        .filtered
        .iter()
        .map(|&ei| icon_request_for_entry(&tab.entries[ei], system_icons))
        .collect();
    let cached_icons: Vec<Option<std::sync::Arc<crate::fs::thumbnail::IconPixels>>> = icon_requests
        .iter()
        .map(|request| {
            request
                .as_ref()
                .and_then(crate::fs::thumbnail::cached_request)
        })
        .collect();

    // Git 状态：当前目录若属于仓库则计算一次工作区状态（虚拟路径跳过）
    let cur_path = tab.history.current().clone();
    let git_info = if crate::fs::virtualfs::is_virtual(&cur_path.to_string_lossy()) {
        None
    } else {
        crate::git::status_for_dir(&cur_path)
    };

    // 文件条目
    let rows: Vec<FileEntry> = tab
        .filtered
        .iter()
        .enumerate()
        .map(|(fi, &ei)| {
            let e = &tab.entries[ei];
            // 缓存命中：直接构建图像预填，否则留空由后台线程回填
            let (thumb, has_thumb) = match &cached_icons[fi] {
                Some(ic) => (image_from(ic), true),
                None => (Image::default(), false),
            };
            let (disk_ratio, disk_info, disk_color) = disk_fields(e, cur_is_this_pc, &disks);
            FileEntry {
                name: e.name.clone().into(),
                path: e.path.clone().into(),
                is_dir: e.is_dir,
                size: metadata::human_size(e.size_bytes).into(),
                size_bytes: e.size_bytes as i32,
                modified: metadata::fmt_ts_label(e.modified_ts).into(),
                modified_ts: e.modified_ts as i32,
                kind: e.kind.clone().into(),
                icon_label: e.icon_label.clone().into(),
                icon_class: e.icon_class.clone().into(),
                selected: tab.selected.get(fi).copied().unwrap_or(false),
                tag_important: core.config.has_tag(&e.path, "important"),
                tag_archive: core.config.has_tag(&e.path, "archive"),
                tag_done: core.config.has_tag(&e.path, "done"),
                git_status: git_info
                    .as_ref()
                    .map(|g| g.status_of(&e.path, e.is_dir))
                    .unwrap_or_default()
                    .into(),
                thumb,
                has_thumb,
                disk_ratio,
                disk_info,
                disk_color,
            }
        })
        .collect();

    // 仅收集"需要图标且缓存未命中"的条目交给后台异步加载。
    let mut jobs: Vec<IconJob> = Vec::new();
    for (fi, request) in icon_requests.into_iter().enumerate() {
        if let Some(request) = request {
            if cached_icons[fi].is_none() {
                jobs.push((fi, request));
            }
        }
    }

    state.set_entries(ModelRc::new(VecModel::from(rows)));

    // 导航到新目录后重置列表滚动位置到顶部（各列表视图监听此 token 变化归零 viewport-y）
    state.set_scroll_top_token(state.get_scroll_top_token() + 1);

    // 每次重建模型自增代数，后台线程据此丢弃过期目录的结果
    let generation = THUMB_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if !jobs.is_empty() {
        spawn_thumbnails(ui, jobs, generation, ThumbSide::Left);
    }

    // 面包屑与标题：虚拟路径使用友好名称
    let cur = tab.history.current();
    let cur_str = cur.to_string_lossy().to_string();
    state.set_current_vpath(cur_str.clone().into());
    if crate::fs::virtualfs::is_virtual(&cur_str) {
        let title = crate::fs::virtualfs::friendly_title(&cur_str);
        let crumbs = vec![Crumb {
            name: title.clone().into(),
            path: cur_str.clone().into(),
        }];
        state.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
        state.set_current_title(title.into());
        let subtitle = format!("{} 个项目", tab.entries.len());
        state.set_current_subtitle(subtitle.into());
    } else {
        let crumbs = build_crumbs(cur);
        state.set_crumbs(ModelRc::new(VecModel::from(crumbs)));

        let title = cur
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| cur.to_string_lossy().to_string());
        state.set_current_title(title.into());

        let dir_count = tab.entries.iter().filter(|e| e.is_dir).count();
        let file_count = tab.entries.len() - dir_count;
        let subtitle = if tab.search.is_empty() {
            format!(
                "{} 个项目 ({} 个文件夹，{} 个文件)",
                tab.entries.len(),
                dir_count,
                file_count
            )
        } else {
            format!(
                "过滤结果：{} / {} 个项目",
                tab.filtered.len(),
                tab.entries.len()
            )
        };
        state.set_current_subtitle(subtitle.into());
    }

    // 导航可用性
    state.set_can_back(tab.history.can_back());
    state.set_can_forward(tab.history.can_forward());

    // 排序状态
    state.set_sort_key(tab.sort_key.clone().into());
    state.set_sort_asc(tab.sort_asc);

    // 当前目录 Git 分支（空串则 UI 隐藏内容头 Git 芯片）
    state.set_git_branch(
        git_info
            .as_ref()
            .map(|g| g.branch.clone())
            .unwrap_or_default()
            .into(),
    );

    // 状态栏与详情：按活动面板同步 sel-*（双面板右面板活动时不能用左面板
    // 选中状态覆盖「sel-* = 最近交互面板」语义——如 watcher 软刷新左目录时）
    update_status(ui, core);
    let right_active = state.get_dual_pane() && state.get_active_pane() == "right";
    update_selection_pane(ui, core, right_active);
}

/// 后台异步加载缩略图/系统图标，逐个回填到 entries 模型。
///
/// 设计：
/// - 单后台线程串行调用 thumbnail::extract（COM 在该线程初始化一次），逐条出图；
/// - 每出一张就通过 upgrade_in_event_loop 回到 UI 线程，由 UI 线程构建
///   SharedPixelBuffer→Image 并就地写回对应行（SharedPixelBuffer 非 Send）；
/// - 回到 UI 线程后先比对 generation：与当前全局代数不一致说明目录已切换，
///   直接丢弃，避免旧目录缩略图错填到新目录。
fn spawn_thumbnails(ui: &MainWindow, jobs: Vec<IconJob>, generation: u64, side: ThumbSide) {
    let weak = ui.as_weak();
    let gen_ref = side.generation();
    std::thread::spawn(move || {
        for (row, request) in jobs {
            // 已切换目录：提前结束本批，省去无谓的图标提取
            if gen_ref.load(Ordering::SeqCst) != generation {
                break;
            }
            // 走带缓存的入口：按类型/文件提取一次并写入缓存，后续同类型条目同步命中
            let icon = crate::fs::thumbnail::load_cached_request(&request, THUMB_SIZE);
            eprintln!(
                "[icon-debug] side={} row={} request={:?} ok={}",
                if side == ThumbSide::Left { "L" } else { "R" },
                row,
                request,
                icon.is_some()
            );
            let Some(icon) = icon else {
                continue;
            };
            let weak2 = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                // 回到 UI 线程：代数校验 + 构建图像 + 就地写回
                if gen_ref.load(Ordering::SeqCst) != generation {
                    return;
                }
                let Some(ui) = weak2.upgrade() else { return };
                let state = ui.global::<AppState>();
                let model = match side {
                    ThumbSide::Left => state.get_entries(),
                    ThumbSide::Right => state.get_r_entries(),
                };
                if let Some(mut entry) = model.row_data(row) {
                    let img = image_from(&icon);
                    entry.thumb = img.clone();
                    entry.has_thumb = true;
                    let is_selected = entry.selected;
                    model.set_row_data(row, entry);
                    // 左侧：若该行正是当前选中项，同步刷新右侧详情面板预览
                    if side == ThumbSide::Left && is_selected {
                        state.set_sel_thumb(img);
                        state.set_sel_has_thumb(true);
                    }
                }
            });
        }
    });
}

/// 把右侧独立面板（双面板视图）的状态推送到 UI 的 r-* 属性。
/// 与左侧一致：按图标来源设置异步加载系统图标/缩略图，回填到 r_entries 模型。
pub fn push_right(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let tab = &core.right_pane;

    // 图标来源与左侧保持一致
    let system_icons = core.config.settings.icon_source == "system";

    // 先同步查缓存预填（与 push_entries 同逻辑）
    let icon_requests: Vec<Option<crate::fs::thumbnail::IconRequest>> = tab
        .filtered
        .iter()
        .map(|&ei| icon_request_for_entry(&tab.entries[ei], system_icons))
        .collect();
    let cached_icons: Vec<Option<std::sync::Arc<crate::fs::thumbnail::IconPixels>>> = icon_requests
        .iter()
        .map(|request| {
            request
                .as_ref()
                .and_then(crate::fs::thumbnail::cached_request)
        })
        .collect();

    let rows: Vec<FileEntry> = tab
        .filtered
        .iter()
        .enumerate()
        .map(|(fi, &ei)| {
            let e = &tab.entries[ei];
            let (thumb, has_thumb) = match &cached_icons[fi] {
                Some(ic) => (image_from(ic), true),
                None => (Image::default(), false),
            };
            FileEntry {
                name: e.name.clone().into(),
                path: e.path.clone().into(),
                is_dir: e.is_dir,
                size: metadata::human_size(e.size_bytes).into(),
                size_bytes: e.size_bytes as i32,
                modified: metadata::fmt_ts_label(e.modified_ts).into(),
                modified_ts: e.modified_ts as i32,
                kind: e.kind.clone().into(),
                icon_label: e.icon_label.clone().into(),
                icon_class: e.icon_class.clone().into(),
                selected: tab.selected.get(fi).copied().unwrap_or(false),
                tag_important: core.config.has_tag(&e.path, "important"),
                tag_archive: core.config.has_tag(&e.path, "archive"),
                tag_done: core.config.has_tag(&e.path, "done"),
                // 右面板暂不展示 Git 徽章
                git_status: SharedString::new(),
                thumb,
                has_thumb,
                // 右面板不展示"此电脑"平铺视图，容量字段恒为默认
                disk_ratio: 0.0,
                disk_info: SharedString::new(),
                disk_color: slint::Brush::SolidColor(slint::Color::from_argb_u8(0, 0, 0, 0)),
            }
        })
        .collect();

    // 仅收集缓存未命中的条目交给后台异步加载
    let mut jobs: Vec<IconJob> = Vec::new();
    for (fi, request) in icon_requests.into_iter().enumerate() {
        if let Some(request) = request {
            if cached_icons[fi].is_none() {
                jobs.push((fi, request));
            }
        }
    }

    state.set_r_entries(ModelRc::new(VecModel::from(rows)));

    // 导航到新目录后重置右面板滚动位置到顶部
    state.set_r_scroll_top_token(state.get_r_scroll_top_token() + 1);

    let generation = R_THUMB_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    if !jobs.is_empty() {
        spawn_thumbnails(ui, jobs, generation, ThumbSide::Right);
    }

    let cur = tab.history.current();
    let cur_str = cur.to_string_lossy().to_string();
    state.set_r_current_vpath(cur_str.clone().into());

    // 右侧面包屑（供双面板共用工具栏地址栏显示）
    let crumbs = build_crumbs(cur);
    state.set_r_crumbs(ModelRc::new(VecModel::from(crumbs)));
    let title = cur
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| cur_str.clone());
    state.set_r_current_title(title.into());

    let dir_count = tab.entries.iter().filter(|e| e.is_dir).count();
    let file_count = tab.entries.len() - dir_count;
    state.set_r_current_subtitle(
        format!(
            "{} 个项目 ({} 个文件夹，{} 个文件)",
            tab.entries.len(),
            dir_count,
            file_count
        )
        .into(),
    );
    state.set_r_can_back(tab.history.can_back());
    state.set_r_can_forward(tab.history.can_forward());
}

/// 仅就地更新右侧面板条目模型的选中标记（保持双击连续触发，不重建模型）
pub fn refresh_right_selection(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let model = state.get_r_entries();
    let tab = &core.right_pane;
    for fi in 0..model.row_count() {
        if let Some(mut row) = model.row_data(fi) {
            let sel = tab.selected.get(fi).copied().unwrap_or(false);
            if row.selected != sel {
                row.selected = sel;
                model.set_row_data(fi, row);
            }
        }
    }
}

/// 构建面包屑链
fn build_crumbs(path: &Path) -> Vec<Crumb> {
    let mut crumbs = Vec::new();
    let mut acc = PathBuf::new();
    let mut comps = path.components().peekable();
    while let Some(comp) = comps.next() {
        acc.push(comp.as_os_str());
        let name = match comp {
            std::path::Component::Prefix(p) => p.as_os_str().to_string_lossy().to_string(),
            std::path::Component::RootDir => continue,
            other => other.as_os_str().to_string_lossy().to_string(),
        };
        let crumb_path = if acc.components().count() == 1 {
            let mut p = acc.clone();
            p.push("\\");
            p
        } else {
            acc.clone()
        };
        crumbs.push(Crumb {
            name: name.into(),
            path: crumb_path.to_string_lossy().to_string().into(),
        });
    }
    crumbs
}

/// 更新状态栏文本
pub fn update_status(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let tab = core.active_tab();
    let sel = tab.selected.iter().filter(|&&s| s).count();
    let total = tab.entries.len();
    let cur = tab.history.current();

    let mut disk_part = String::new();
    if let Some(letter) = cur.to_string_lossy().chars().next() {
        for d in disk::list_disks() {
            if d.letter.eq_ignore_ascii_case(&letter.to_string()) {
                disk_part = format!(" · {} 剩余 {}", d.name, metadata::human_size(d.free));
                break;
            }
        }
    }

    let text = if sel > 0 {
        format!("已选择 {} 项，共 {} 项{}", sel, total, disk_part)
    } else {
        // 无选中时在左下角展示文件夹 / 文件数量明细
        let dir_count = tab.entries.iter().filter(|e| e.is_dir).count();
        let file_count = total - dir_count;
        if tab.search.is_empty() {
            format!(
                "共 {} 项 ({} 个文件夹，{} 个文件){}",
                total, dir_count, file_count, disk_part
            )
        } else {
            format!(
                "过滤结果：{} / {} 项{}",
                tab.filtered.len(),
                total,
                disk_part
            )
        }
    };
    state.set_status_text(text.into());
}

/// 更新详情面板与选中信息（活动标签 = 左面板）
pub fn update_selection(ui: &MainWindow, core: &AppCore) {
    update_selection_pane(ui, core, false);
}

/// 更新详情面板与选中信息，按面板取数：`right` 为真时数据源为右侧独立面板。
/// sel-* 全局属性语义为「最近交互面板的选中项」，属性对话框 / 详情栏 /
/// ActionBar「解压」按钮可见性均由此驱动。
pub fn update_selection_pane(ui: &MainWindow, core: &AppCore, right: bool) {
    let state = ui.global::<AppState>();
    let tab = core.pane(right);
    let sel_idx: Vec<usize> = tab
        .selected
        .iter()
        .enumerate()
        .filter(|(_, &s)| s)
        .map(|(i, _)| i)
        .collect();
    // 选中数量：驱动 ActionBar 按钮可用性（复制/剪切/删除等需 >0，重命名需 ==1）
    state.set_sel_count(sel_idx.len() as i32);

    // 工具栏「标记」下拉对勾：选中项是否「全部」含某标签（空选中则为否）。
    let all_tag = |key: &str| {
        !sel_idx.is_empty()
            && sel_idx.iter().all(|&fi| {
                tab.entry_at(fi)
                    .map(|e| core.config.has_tag(&e.path, key))
                    .unwrap_or(false)
            })
    };
    state.set_sel_tag_important(all_tag("important"));
    state.set_sel_tag_archive(all_tag("archive"));
    state.set_sel_tag_done(all_tag("done"));

    if sel_idx.len() == 1 {
        if let Some(e) = tab.entry_at(sel_idx[0]) {
            state.set_has_selection(true);
            state.set_sel_is_archive(crate::fs::operations::is_zip_archive(Path::new(&e.path)));
            state.set_sel_is_dir(e.is_dir);
            // 切换选中项时重置文件夹大小计算状态：新文件夹需重新点「计算」
            state.set_sel_size_calculating(false);
            state.set_sel_path(e.path.clone().into());
            // 设置开启「计算文件夹大小」时选中即自动后台统计（大文件夹期间显示"计算中"）
            if e.is_dir && core.config.settings.calc_folder_size {
                state.invoke_calculate_folder_size();
            }
            state.set_sel_name(e.name.clone().into());
            state.set_sel_kind(e.kind.clone().into());
            let loc = Path::new(&e.path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            state.set_sel_location(loc.into());
            state.set_sel_size(
                if e.is_dir {
                    "—".to_string()
                } else {
                    format!(
                        "{} ({} 字节)",
                        metadata::human_size(e.size_bytes),
                        e.size_bytes
                    )
                }
                .into(),
            );
            state.set_sel_modified(metadata::fmt_ts_full(e.modified_ts).into());
            // 创建时间：条目模型未缓存，按需对单个文件读取一次
            let created = std::fs::metadata(&e.path)
                .ok()
                .and_then(|m| m.created().ok())
                .map(metadata::full_time)
                .unwrap_or_else(|| "—".to_string());
            state.set_sel_created(created.into());
            // 「打开方式」默认程序名（仅文件；文件夹/无关联留空，UI 据此隐藏该行）
            let open_with = if e.is_dir {
                String::new()
            } else {
                crate::fs::openwith::default_app_name(Path::new(&e.path)).unwrap_or_default()
            };
            state.set_sel_open_with(open_with.into());
            // 安全页：所有者 + 属性摘要 + 只读标志
            fill_security(&state, Path::new(&e.path));
            // 数字签名页
            fill_signature(&state, Path::new(&e.path), e.is_dir);
            state.set_sel_icon_label(e.icon_label.clone().into());
            state.set_sel_icon_class(e.icon_class.clone().into());
            // 从对应面板的条目模型读取该行已生成的缩略图/系统图标位图，填充右侧详情预览
            let model = if right {
                state.get_r_entries()
            } else {
                state.get_entries()
            };
            if let Some(row) = model.row_data(sel_idx[0]) {
                state.set_sel_has_thumb(row.has_thumb);
                state.set_sel_thumb(row.thumb.clone());
            } else {
                state.set_sel_has_thumb(false);
            }
            return;
        }
    }
    state.set_has_selection(false);
    state.set_sel_is_archive(false);
    state.set_sel_is_dir(false);
    state.set_sel_size_calculating(false);
    state.set_sel_has_thumb(false);
}

/// 仅就地更新条目模型的选中标记，避免重建 entries 模型导致
/// `for` 元素被销毁、双击事件无法连续触发的问题。
pub fn refresh_selection(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let model = state.get_entries();
    let tab = core.active_tab();
    for fi in 0..model.row_count() {
        if let Some(mut row) = model.row_data(fi) {
            let sel = tab.selected.get(fi).copied().unwrap_or(false);
            if row.selected != sel {
                row.selected = sel;
                model.set_row_data(fi, row);
            }
        }
    }
    update_status(ui, core);
    update_selection(ui, core);
}

/// 构建侧边栏导航项（真实磁盘 + 系统目录 + 标签）
/// `collapsed` 为当前已折叠的分区标签集合，折叠分区下的条目不再生成。
/// `config` 提供标签计数与高亮状态。
pub fn build_sidebar(
    active_path: &Path,
    collapsed: &HashSet<String>,
    config: &crate::config::AppConfig,
) -> ModelRc<NavItem> {
    let mut items: Vec<NavItem> = Vec::new();
    let active_str = active_path.to_string_lossy().to_string();

    // 当前所在分区是否被折叠（随 header 推进而更新）
    let mut section_collapsed;

    // 图标来源：system=系统图标（侧栏也读取系统图标），builtin=内置矢量/字符图标
    let system_icons = config.settings.icon_source == "system";

    let mk = |label: &str, path: String, icon: &str, badge: &str, active: bool| {
        let (thumb, has_thumb) = if system_icons {
            sidebar_icon(&path, true)
        } else {
            (Image::default(), false)
        };
        NavItem {
            label: label.into(),
            path: path.into(),
            icon: icon.into(),
            icon_class: "".into(),
            badge: badge.into(),
            is_header: false,
            is_disk: false,
            disk_ratio: 0.0,
            disk_info: "".into(),
            disk_color: slint::Brush::SolidColor(slint::Color::from_rgb_u8(0, 120, 212)),
            active,
            collapsed: false,
            is_tree: false,
            thumb,
            has_thumb: has_thumb,
        }
    };
    let header = |label: &str, is_collapsed: bool| NavItem {
        label: label.into(),
        path: "".into(),
        icon: "".into(),
        icon_class: "".into(),
        badge: "".into(),
        is_header: true,
        is_disk: false,
        disk_ratio: 0.0,
        disk_info: "".into(),
        disk_color: slint::Brush::default(),
        active: false,
        collapsed: is_collapsed,
        is_tree: false,
        thumb: Image::default(),
        has_thumb: false,
    };

    // 快速访问：优先读取系统真实数据（与资源管理器一致，含固定文件夹 + 常用文件夹）
    section_collapsed = collapsed.contains("快速访问");
    items.push(header("快速访问", section_collapsed));
    if !section_collapsed {
        // 已知系统文件夹 → 专属 MDL2 图标（Segoe MDL2 Assets 字体渲染）；
        // 用户固定的普通文件夹保持原有图标（系统位图 / 首字符色块）
        let known: Vec<(String, &str)> = [
            (dirs::desktop_dir(), "\u{E7F8}"),  // 桌面（显示器）
            (dirs::download_dir(), "\u{E896}"), // 下载
            (dirs::document_dir(), "\u{E8A5}"), // 文档
            (dirs::picture_dir(), "\u{EB9F}"),  // 图片
            (dirs::audio_dir(), "\u{E8D6}"),    // 音乐
            (dirs::video_dir(), "\u{E714}"),    // 视频
        ]
        .into_iter()
        .filter_map(|(p, g)| {
            p.map(|p| (p.to_string_lossy().trim_end_matches('\\').to_lowercase(), g))
        })
        .collect();
        // 已知系统文件夹的图标策略：
        // - 系统图标模式：按路径提取 Shell 专属图标（桌面/下载/文档等在系统中带标识），
        //   与资源管理器显示一致；提取失败回退 MDL2 glyph
        // - 内置图标模式：MDL2 字体 glyph（现有内置样式）
        let glyphize = |mut item: NavItem, glyph: &str| {
            if system_icons {
                if let Some(ic) =
                    crate::fs::thumbnail::special_dir_icon_cached(item.path.as_str(), THUMB_SIZE)
                {
                    item.thumb = image_from(&ic);
                    item.has_thumb = true;
                    return item;
                }
            }
            item.icon = glyph.into();
            item.icon_class = "qa-glyph".into();
            item.has_thumb = false;
            item
        };
        let qa = crate::fs::quickaccess::list();
        if qa.is_empty() {
            // 回退：系统标准目录（非 Windows 或读取失败时仍可用）
            let dirs = [
                ("桌面", dirs::desktop_dir(), "\u{E7F8}"),
                ("下载", dirs::download_dir(), "\u{E896}"),
                ("文档", dirs::document_dir(), "\u{E8A5}"),
                ("图片", dirs::picture_dir(), "\u{EB9F}"),
                ("音乐", dirs::audio_dir(), "\u{E8D6}"),
                ("视频", dirs::video_dir(), "\u{E714}"),
            ];
            if let Some(home) = dirs::home_dir() {
                let hs = home.to_string_lossy().to_string();
                let active = active_str == hs;
                items.push(glyphize(mk("主目录", hs, "", "", active), "\u{E80F}"));
            }
            for (label, dir, glyph) in dirs.iter() {
                if let Some(p) = dir {
                    let ps = p.to_string_lossy().to_string();
                    let active = active_str == ps;
                    items.push(glyphize(mk(label, ps, "", "", active), glyph));
                }
            }
        } else {
            for it in qa {
                let active = active_str == it.path;
                let hit = known
                    .iter()
                    .find(|(p, _)| *p == it.path.trim_end_matches('\\').to_lowercase())
                    .map(|(_, g)| *g);
                if let Some(glyph) = hit {
                    items.push(glyphize(
                        mk(&it.name, it.path.clone(), "", "", active),
                        glyph,
                    ));
                } else {
                    // 图标取显示名首字符（中文文件夹名友好）
                    let icon = it
                        .name
                        .chars()
                        .next()
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    items.push(mk(&it.name, it.path.clone(), &icon, "", active));
                }
            }
        }
    }

    // 此电脑：可点击导航的树节点（点文字进入"此电脑"界面，点 chevron 展开/折叠），
    // 展开后列出所有驱动器（含使用率进度条）与已识别的便携设备。
    section_collapsed = collapsed.contains("此电脑");
    let this_pc_active = active_str == crate::fs::virtualfs::THIS_PC_PATH;
    items.push(NavItem {
        label: "此电脑".into(),
        path: crate::fs::virtualfs::THIS_PC_PATH.into(),
        icon: "电".into(),
        icon_class: "".into(),
        badge: "".into(),
        is_header: false,
        is_disk: false,
        disk_ratio: 0.0,
        disk_info: "".into(),
        disk_color: slint::Brush::SolidColor(slint::Color::from_rgb_u8(0, 120, 212)),
        active: this_pc_active,
        collapsed: section_collapsed,
        is_tree: true,
        thumb: Image::default(),
        has_thumb: false,
    });
    if !section_collapsed {
        // 驱动器：用真实容量构建带使用率进度条的磁盘条目
        for disk in crate::fs::disk::list_disks() {
            let ratio = disk.used_ratio();
            // 与资源管理器一致的副标题："X 可用，共 Y"
            let info = if disk.total > 0 {
                format!(
                    "{} 可用，共 {}",
                    metadata::human_size(disk.free),
                    metadata::human_size(disk.total)
                )
            } else {
                String::new()
            };
            // 使用率 >90% 标红警示，否则蓝色
            let bar = if ratio > 0.9 {
                slint::Color::from_rgb_u8(0xd1, 0x34, 0x38)
            } else {
                slint::Color::from_rgb_u8(0x00, 0x78, 0xd4)
            };
            // 系统图标模式下提取盘符真实图标
            let (disk_thumb, disk_has_thumb) = if system_icons {
                sidebar_icon(&disk.root, true)
            } else {
                (Image::default(), false)
            };
            items.push(NavItem {
                label: disk.name.clone().into(),
                path: disk.root.clone().into(),
                icon: disk.letter.clone().into(),
                icon_class: "drive".into(),
                badge: "".into(),
                is_header: false,
                is_disk: true,
                disk_ratio: ratio,
                disk_info: info.into(),
                disk_color: slint::Brush::SolidColor(bar),
                active: active_str == disk.root,
                collapsed: false,
                is_tree: false,
                thumb: disk_thumb,
                has_thumb: disk_has_thumb,
            });
        }
        // 便携设备（手机 / 平板等）：同步显示在"此电脑"下
        for dev in crate::fs::devices::list_devices() {
            let (device_thumb, device_has_thumb) = if system_icons {
                match crate::fs::thumbnail::load_cached_request(
                    &crate::fs::thumbnail::IconRequest::Device,
                    THUMB_SIZE,
                ) {
                    Some(icon) => {
                        eprintln!("[icon-debug] sidebar device icon OK");
                        (image_from(&icon), true)
                    }
                    None => {
                        eprintln!("[icon-debug] sidebar device icon NONE");
                        (Image::default(), false)
                    }
                }
            } else {
                (Image::default(), false)
            };
            items.push(NavItem {
                label: dev.name.clone().into(),
                path: dev.path.clone().into(),
                icon: "机".into(),
                icon_class: "device".into(),
                badge: "".into(),
                is_header: false,
                is_disk: false,
                disk_ratio: 0.0,
                disk_info: dev.kind.clone().into(),
                disk_color: slint::Brush::SolidColor(slint::Color::from_rgb_u8(0x2a, 0x9d, 0x8f)),
                active: active_str == dev.path,
                collapsed: false,
                is_tree: false,
                thumb: device_thumb,
                has_thumb: device_has_thumb,
            });
        }
    }

    // 标签（真实计数 + tag:// 虚拟路径）
    section_collapsed = collapsed.contains("标签");
    items.push(header("标签", section_collapsed));
    if !section_collapsed {
        let tag_defs = [
            ("important", "重要", (0xd1u8, 0x34u8, 0x38u8)),
            ("archive", "待归档", (0xff, 0x8c, 0x00)),
            ("done", "已完成", (0x10, 0x7c, 0x10)),
        ];
        for (key, label, (r, g, b)) in tag_defs {
            let vpath = format!("tag://{}", key);
            let cnt = config.count(key);
            let badge = if cnt > 0 {
                cnt.to_string()
            } else {
                String::new()
            };
            items.push(NavItem {
                label: label.into(),
                path: vpath.clone().into(),
                icon: "●".into(),
                icon_class: "".into(),
                badge: badge.into(),
                is_header: false,
                is_disk: false,
                disk_ratio: 0.0,
                disk_info: "".into(),
                disk_color: slint::Brush::SolidColor(slint::Color::from_rgb_u8(r, g, b)),
                active: active_str == vpath,
                collapsed: false,
                is_tree: false,
                thumb: Image::default(),
                has_thumb: false,
            });
        }
    }

    // 系统（回收站 / 网络位置，均为虚拟路径，应用内浏览）
    section_collapsed = collapsed.contains("系统");
    items.push(header("系统", section_collapsed));
    if !section_collapsed {
        // 真实系统图标（Stock 图标）：回收站按空/满取对应图标（跟随系统状态变化），
        // 网络位置取「网络」图标；提取失败回退到 MDL2 矢量字形（非文字色块）。
        let mk_sys = |label: &str, path: &str, stock, glyph: &str, active: bool| {
            let mut item = mk(label, path.to_string(), "", "", active);
            match crate::fs::thumbnail::stock_icon_cached(stock, THUMB_SIZE) {
                Some(icon) => {
                    item.thumb = image_from(&icon);
                    item.has_thumb = true;
                }
                None => {
                    item.icon = glyph.into();
                    item.icon_class = "qa-glyph".into();
                    item.has_thumb = false;
                }
            }
            item
        };
        let recycle_stock = match crate::fs::recyclebin::is_empty() {
            Some(false) => crate::fs::thumbnail::StockIcon::RecyclerFull,
            _ => crate::fs::thumbnail::StockIcon::RecyclerEmpty,
        };
        items.push(mk_sys(
            "回收站",
            "recycle://",
            recycle_stock,
            "\u{E74D}",
            active_str == "recycle://",
        ));
        items.push(mk_sys(
            "网络位置",
            "network://",
            crate::fs::thumbnail::StockIcon::Network,
            "\u{E968}",
            active_str == "network://",
        ));
    }

    ModelRc::new(VecModel::from(items))
}

/// 把已保存的网络位置推送到设置「云存储账号」页的列表模型
pub fn push_network_locations(ui: &MainWindow, core: &AppCore) {
    let items: Vec<NetAccount> = core
        .config
        .network_locations
        .iter()
        .map(|l| NetAccount {
            name: l.name.clone().into(),
            server: l.server.clone().into(),
            drive: l.drive.clone().unwrap_or_default().into(),
        })
        .collect();
    ui.global::<AppState>()
        .set_net_locations(ModelRc::new(VecModel::from(items)));
}

/// 供哈希回调返回 SharedString
pub fn hash_to_shared(path: &Path, algo: &str) -> SharedString {
    match crate::fs::hash::compute(path, algo) {
        Ok(v) => v.into(),
        Err(e) => format!("计算失败：{}", e).into(),
    }
}

/// 供校验回调返回 (结果文案, 状态码)。状态码：1 匹配 / 2 不匹配 / 3 错误
pub fn verify_result(path: &Path, expected: &str) -> (SharedString, i32) {
    match crate::fs::hash::verify(path, expected) {
        Ok((algo, true)) => (format!("匹配（{}）", algo).into(), 1),
        Ok((algo, false)) => (format!("不匹配（已按 {} 比对）", algo).into(), 2),
        Err(e) => (format!("校验失败：{}", e).into(), 3),
    }
}

/// 填充属性「安全」页：所有者 + 属性摘要 + 只读标志。
/// 所有者读取依赖平台 API，非 Windows 平台留作占位。
fn fill_security(state: &AppState, path: &Path) {
    let meta = std::fs::metadata(path).ok();
    // 只读标志：跨平台可用（permissions.readonly）
    let readonly = meta
        .as_ref()
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);
    state.set_sel_readonly(readonly);

    // 属性摘要：只读 / 隐藏（隐藏判断为 Windows 专属，其它平台按文件名以 . 开头近似）
    let mut attrs: Vec<&str> = Vec::new();
    if readonly {
        attrs.push("只读");
    }
    let hidden = is_hidden(path);
    if hidden {
        attrs.push("隐藏");
    }
    let attr_text = if attrs.is_empty() {
        "普通".to_string()
    } else {
        attrs.join("、")
    };
    state.set_sel_attributes(attr_text.into());

    // 所有者
    let owner = file_owner(path).unwrap_or_else(|| "—".to_string());
    state.set_sel_owner(owner.into());

    // DACL 访问控制项
    let aces: Vec<AclAce> = crate::fs::openwith::acl_entries(path)
        .into_iter()
        .map(|(trustee, kind, access)| AclAce {
            trustee: trustee.into(),
            kind: kind.into(),
            access: access.into(),
        })
        .collect();
    state.set_sel_acl_entries(ModelRc::new(VecModel::from(aces)));
}

/// 填充属性「详细信息」页：按文件类型解析扩展元数据（EXIF / 音频标签 / Office 核心属性）。
/// 在打开属性对话框时一次性填充，避免每次切换选中项都解析。
pub fn fill_details(state: &AppState, path: &Path) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let mut rows: Vec<MetaRow> = Vec::new();
    match ext.as_str() {
        "jpg" | "jpeg" | "tiff" | "tif" | "heic" | "webp" => rows.extend(read_exif(path)),
        "mp3" | "flac" | "m4a" | "ogg" | "wav" | "aac" | "wma" => rows.extend(read_audio(path)),
        "docx" | "xlsx" | "pptx" => rows.extend(read_office(path)),
        _ => {}
    }
    if rows.is_empty() {
        rows.push(MetaRow {
            label: "说明".into(),
            value: "此文件类型无扩展元数据".into(),
        });
    }
    state.set_sel_meta_rows(ModelRc::new(VecModel::from(rows)));
}

/// 读取图片 EXIF 信息（相机型号、拍摄参数等）。
fn read_exif(path: &Path) -> Vec<MetaRow> {
    use exif::{In, Reader, Tag};
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut buf = std::io::BufReader::new(&file);
    let exif = match Reader::new().read_from_container(&mut buf) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let tags: [(Tag, &str); 9] = [
        (Tag::Make, "相机制造商"),
        (Tag::Model, "相机型号"),
        (Tag::DateTime, "拍摄时间"),
        (Tag::ExposureTime, "曝光时间"),
        (Tag::FNumber, "光圈"),
        (Tag::ISOSpeed, "ISO 感光度"),
        (Tag::FocalLength, "焦距"),
        (Tag::PixelXDimension, "图像宽度"),
        (Tag::PixelYDimension, "图像高度"),
    ];
    let mut rows = Vec::new();
    for (tag, label) in tags {
        if let Some(f) = exif.get_field(tag, In::PRIMARY) {
            let v = format!("{}", f.display_value().with_unit(&exif));
            rows.push(MetaRow {
                label: label.into(),
                value: v.into(),
            });
        }
    }
    rows
}

/// 读取音频标签（标题、艺术家、专辑、时长等）。
fn read_audio(path: &Path) -> Vec<MetaRow> {
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::tag::ItemKey;
    let tf = match lofty::probe::read_from_path(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    let dur = tf.properties().duration();
    rows.push(MetaRow {
        label: "时长".into(),
        value: format!("{}:{:02}", dur.as_secs() / 60, dur.as_secs() % 60).into(),
    });
    if let Some(tag) = tf.primary_tag() {
        for (key, label) in [
            (ItemKey::TrackTitle, "标题"),
            (ItemKey::TrackArtist, "艺术家"),
            (ItemKey::AlbumTitle, "专辑"),
            (ItemKey::Year, "年份"),
            (ItemKey::Genre, "流派"),
        ] {
            if let Some(v) = tag.get_string(&key) {
                rows.push(MetaRow {
                    label: label.into(),
                    value: v.to_string().into(),
                });
            }
        }
    }
    rows
}

/// 读取 Office 文档（docx/xlsx/pptx）核心属性：标题、作者、修改者、创建/修改时间。
/// OOXML 是 ZIP 包，docProps/core.xml 存核心属性（Dublin Core 命名空间）。
fn read_office(path: &Path) -> Vec<MetaRow> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let mut xml = String::new();
    for i in 0..archive.len() {
        if let Ok(mut f) = archive.by_index(i) {
            if f.name() == "docProps/core.xml" {
                let _ = f.read_to_string(&mut xml);
                break;
            }
        }
    }
    if xml.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for (tag, label) in [
        ("dc:title", "标题"),
        ("dc:creator", "作者"),
        ("cp:lastModifiedBy", "最后修改者"),
        ("dcterms:created", "创建时间"),
        ("dcterms:modified", "修改时间"),
    ] {
        if let Some(v) = extract_xml_tag(&xml, tag) {
            rows.push(MetaRow {
                label: label.into(),
                value: v.into(),
            });
        }
    }
    rows
}

/// 从 XML 文本中提取 `<tag ...>value</tag>` 的内容（简单字符串解析，避免引入 XML 库）。
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)?;
    let gt = xml[start..].find('>')? + start + 1;
    let end = xml[gt..].find(&close)? + gt;
    Some(xml[gt..end].trim().to_string())
}

/// 判断文件是否隐藏。Windows 读取隐藏属性位；其它平台按 . 前缀近似。
#[cfg(windows)]
fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    std::fs::metadata(path)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

/// 读取文件所有者名称（Windows：Shell 取 System.FileOwner；其它平台返回 None）。
#[cfg(windows)]
fn file_owner(path: &Path) -> Option<String> {
    crate::fs::openwith::file_owner(path)
}

#[cfg(not(windows))]
fn file_owner(_path: &Path) -> Option<String> {
    None
}

/// 填充属性「数字签名」页：状态码 + 说明文本。
/// 状态码：0 不适用 / 1 已签名 / 2 未签名 / 3 错误
fn fill_signature(state: &AppState, path: &Path, is_dir: bool) {
    use crate::fs::signature::{cert_chain, detect, SignStatus};
    // 默认清空证书链（仅 Signed 分支填充）
    state.set_sel_cert_chain(ModelRc::new(VecModel::from(Vec::<CertInfo>::new())));
    if is_dir {
        state.set_sel_sign_status(0);
        state.set_sel_sign_detail("文件夹没有数字签名。".into());
        return;
    }
    match detect(path) {
        SignStatus::NotApplicable => {
            state.set_sel_sign_status(0);
            state.set_sel_sign_detail("该文件类型通常不包含数字签名。".into());
        }
        SignStatus::Signed { cert_bytes } => {
            state.set_sel_sign_status(1);
            state.set_sel_sign_detail(
                format!(
                    "检测到内嵌 Authenticode 签名（证书数据 {}）。",
                    metadata::human_size(cert_bytes as u64)
                )
                .into(),
            );
            let chain: Vec<CertInfo> = cert_chain(path)
                .into_iter()
                .map(|(s, i, f, t)| CertInfo {
                    subject: s.into(),
                    issuer: i.into(),
                    valid_from: f.into(),
                    valid_to: t.into(),
                })
                .collect();
            state.set_sel_cert_chain(ModelRc::new(VecModel::from(chain)));
        }
        SignStatus::Unsigned => {
            state.set_sel_sign_status(2);
            state.set_sel_sign_detail("可执行文件，但未找到内嵌数字签名。".into());
        }
        SignStatus::Error(e) => {
            state.set_sel_sign_status(3);
            state.set_sel_sign_detail(format!("读取签名信息失败：{}", e).into());
        }
    }
}

/// Quick Look 大图提取尺寸（图片预览用，比列表缩略图更大更清晰）
// 预览位图提取上限：更高的源分辨率让滚轮放大查看时保持清晰
const QL_IMAGE_SIZE: u32 = 1600;

/// 填充 Quick Look 预览内容：根据选中项类型设置 ql-* 属性。
/// `right` 为真时预览右侧面板的选中项（双面板右侧活动）。
/// 返回是否成功设置（无选中项返回 false，调用方据此决定是否打开浮层）。
pub fn fill_quicklook(ui: &MainWindow, core: &AppCore, right: bool) -> bool {
    use crate::fs::preview::{self, PreviewKind};
    let state = ui.global::<AppState>();
    let tab = core.pane(right);
    let Some(fi) = tab.first_selected() else {
        return false;
    };
    let Some(e) = tab.entry_at(fi) else {
        return false;
    };

    let path = Path::new(&e.path);
    let kind = preview::kind_of(path, e.is_dir);
    state.set_ql_kind(kind.code());
    state.set_ql_name(e.name.clone().into());
    state.set_ql_icon_class(e.icon_class.clone().into());
    // 默认清空上一次的图片/文本，避免切换文件时残留旧内容
    state.set_ql_has_image(false);
    state.set_ql_text("".into());
    state.set_ql_code_kw("".into());
    state.set_ql_code_str("".into());
    state.set_ql_code_cmt("".into());
    state.set_ql_info("".into());
    // 条目的真实缩略图/系统图标（取对应面板列表模型已生成的位图），头部与大图标优先显示
    let model = if right {
        state.get_r_entries()
    } else {
        state.get_entries()
    };
    if let Some(row) = model.row_data(fi) {
        state.set_ql_thumb(row.thumb.clone());
        state.set_ql_has_thumb(row.has_thumb);
    } else {
        state.set_ql_has_thumb(false);
    }

    let size_text = if e.is_dir {
        String::new()
    } else {
        format!("{} · {}", e.kind, metadata::human_size(e.size_bytes))
    };

    match kind {
        PreviewKind::Image => {
            // 真实像素尺寸：仅解析文件头，不解码整图
            let (iw, ih) = imagesize::size(path)
                .map(|d| (d.width as i32, d.height as i32))
                .unwrap_or((0, 0));
            // 复用缩略图提取，按更大尺寸取清晰位图（缩放查看时仍然清晰）
            #[cfg(windows)]
            if let Some(icon) = crate::fs::thumbnail::extract(&e.path, QL_IMAGE_SIZE)
                .map(|(pixels, w, h)| crate::fs::thumbnail::IconPixels { pixels, w, h })
            {
                // 文件头解析失败（如损坏/罕见格式）时回退位图自身尺寸
                let (iw, ih) = if iw > 0 {
                    (iw, ih)
                } else {
                    (icon.w as i32, icon.h as i32)
                };
                state.set_ql_img_w(iw);
                state.set_ql_img_h(ih);
                state.set_ql_subtitle(
                    format!(
                        "{}×{} 像素 · {}",
                        iw,
                        ih,
                        metadata::human_size(e.size_bytes)
                    )
                    .into(),
                );
                state.set_ql_image(image_from(&icon));
                state.set_ql_has_image(true);
            } else {
                state.set_ql_img_w(iw);
                state.set_ql_img_h(ih);
                state.set_ql_subtitle(size_text.into());
            }
            #[cfg(not(windows))]
            {
                state.set_ql_img_w(iw);
                state.set_ql_img_h(ih);
                state.set_ql_subtitle(size_text.into());
            }
        }
        PreviewKind::Text => {
            state.set_ql_subtitle(size_text.into());
            // 读取首部 64KB 文本并按扩展名做分层语法高亮
            let text = preview::read_text_head(path, 64 * 1024);
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let layers = crate::fs::highlight::highlight(&text, &ext);
            state.set_ql_text(layers.base.into());
            state.set_ql_code_kw(layers.keywords.into());
            state.set_ql_code_str(layers.strings.into());
            state.set_ql_code_cmt(layers.comments.into());
        }
        PreviewKind::Video => {
            // 视频：内容由 Media Foundation 子窗口渲染（main.rs 打开预览时启动），
            // 这里只填标题信息
            state.set_ql_subtitle(size_text.into());
        }
        PreviewKind::Folder => {
            let (dirs, files, fsize) = preview::folder_summary(path);
            state.set_ql_subtitle("文件夹".into());
            state.set_ql_info(
                format!(
                    "包含 {} 个子文件夹、{} 个文件\n顶层文件合计 {}",
                    dirs,
                    files,
                    metadata::human_size(fsize)
                )
                .into(),
            );
        }
        PreviewKind::Info => {
            state.set_ql_subtitle(size_text.into());
            state.set_ql_info(
                format!(
                    "{}\n位置：{}\n修改时间：{}",
                    e.kind,
                    Path::new(&e.path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    metadata::fmt_ts_full(e.modified_ts)
                )
                .into(),
            );
        }
    }
    true
}

/// 推送标签页列表到 UI
pub fn push_tabs(ui: &MainWindow, core: &AppCore) {
    let state = ui.global::<AppState>();
    let rows: Vec<TabInfo> = core
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| TabInfo {
            title: t.title().into(),
            path: t.history.current().to_string_lossy().to_string().into(),
            active: i == core.active,
            kind: (if t.kind == TabKind::Settings {
                "settings"
            } else {
                "files"
            })
            .into(),
        })
        .collect();
    state.set_tabs(ModelRc::new(VecModel::from(rows)));
    state.set_active_tab(core.active as i32);

    // 主体视图随活动标签页类型切换（设置页 / 文件浏览）
    let view = if core.active_tab().kind == TabKind::Settings {
        "settings"
    } else {
        "files"
    };
    state.set_active_view(view.into());
}

#[cfg(test)]
mod icon_request_tests {
    use super::*;
    use crate::fs::thumbnail::IconRequest;

    fn entry(name: &str, path: &str, is_dir: bool, icon_class: &str) -> metadata::Entry {
        metadata::Entry {
            name: name.into(),
            path: path.into(),
            is_dir,
            size_bytes: 0,
            modified_ts: 7,
            kind: String::new(),
            icon_label: String::new(),
            icon_class: icon_class.into(),
        }
    }

    #[test]
    fn device_entries_never_become_real_path_requests() {
        let device = entry("手机", "device://id", true, "device");
        assert_eq!(
            icon_request_for_entry(&device, true),
            Some(IconRequest::Device)
        );
        assert_eq!(icon_request_for_entry(&device, false), None);

        let file = entry("报告.PDF", "device://id\u{1}object", false, "document");
        assert_eq!(
            icon_request_for_entry(&file, true),
            Some(IconRequest::Type {
                extension: "PDF".into(),
                is_dir: false,
            })
        );
    }

    #[test]
    fn local_and_other_virtual_entries_keep_existing_policy() {
        let local = entry("readme.txt", r"C:\readme.txt", false, "document");
        assert!(matches!(
            icon_request_for_entry(&local, true),
            Some(IconRequest::RealPath { .. })
        ));
        assert_eq!(icon_request_for_entry(&local, false), None);

        let tag = entry("重要", "tag://important", true, "folder");
        assert_eq!(icon_request_for_entry(&tag, true), None);
    }
}

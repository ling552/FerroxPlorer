// FerroxPlorer 入口：初始化主窗口、多标签页、绑定全部回调到真实文件系统
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod fs;
mod git;
mod ui_bridge;
mod update;

slint::include_modules!();

use app::{AppCore, ClipMode};
use fs::operations as ops;
use slint::ComponentHandle;
use slint::Model;
// 无边框窗口下访问底层 winit 窗口以实现自定义标题栏拖动
use slint::winit_030::WinitWindowAccessor;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn home_start_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("C:\\"))
}

fn startup_path(setting: &str) -> PathBuf {
    match setting {
        "this-pc" => PathBuf::from(fs::virtualfs::THIS_PC_PATH),
        "quick" | "last" => home_start_path(),
        _ => home_start_path(),
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // 安装程序卸载前调用：不创建窗口，只安全恢复仍由本应用持有的 Shell 关联。
    if std::env::args_os().any(|arg| arg == "--unregister-default-file-manager") {
        let _ = fs::default_app::set_default(false);
        return Ok(());
    }

    // 抑制 Slint 文本分词触发的 ICU4X "No segmentation model for language: ja"
    // 警告刷屏。
    log::set_max_level(log::LevelFilter::Error);

    let ui = MainWindow::new()?;

    // 启动目录：命令行参数优先（作为默认文件管理器被系统调起时传入目标目录），
    // 否则按用户设置进入，此电脑映射到虚拟驱动器根。
    // 用 args_os 避免非法 Unicode 路径（NTFS 允许未配对代理项）导致 panic。
    let startup_config = config::AppConfig::load();
    let arg_dir = std::env::args_os()
        .nth(1)
        .map(|os| {
            let s = os.to_string_lossy().into_owned();
            // 经典转义修复：shell 展开 "%1" 为 "D:\" 时，尾反斜杠+引号被
            // CommandLineToArgvW 解析成字面引号（参数变为 D:"）——去掉尾引号，
            // 纯盘符（"D:"）补回反斜杠成盘根
            let s = s.trim_end_matches('"').to_string();
            if s.len() == 2 && s.ends_with(':') {
                PathBuf::from(format!("{}\\", s))
            } else {
                PathBuf::from(s)
            }
        })
        .filter(|p| p.is_dir());
    let start = arg_dir.unwrap_or_else(|| startup_path(&startup_config.settings.startup_open));
    // 「默认文件管理器」自愈仅修复带本应用所有权标记的缺失项或旧安装路径；
    // 若用户后来改用其它文件管理器，不会静默夺回关联。
    let default_fm_state = if startup_config.settings.default_file_manager {
        fs::default_app::registration_state()
    } else {
        fs::default_app::RegistrationState::Disabled
    };
    let default_fm_repair_error = if startup_config.settings.default_file_manager
        && default_fm_state == fs::default_app::RegistrationState::Repairable
    {
        fs::default_app::set_default(true).err()
    } else {
        None
    };
    let core = Rc::new(RefCell::new(AppCore::new(start.clone())));
    core.borrow_mut().config = startup_config;

    // 首次加载
    load_current(&ui, &core);
    // 右侧独立面板首次加载（双面板视图用）
    load_right(&ui, &core);
    let state = ui.global::<AppState>();
    {
        let c = core.borrow();
        state.set_nav_items(ui_bridge::build_sidebar(
            &start,
            &c.collapsed_sections,
            &c.config,
        ));
    }

    // 启动时把持久化的用户设置推送到 Theme 与 AppState
    push_settings(&ui, &core);
    if default_fm_state == fs::default_app::RegistrationState::External {
        // 其它程序已接管至少一个入口：安全恢复本应用仍持有的其余入口，再关闭配置开关。
        let _ = fs::default_app::set_default(false);
        state.set_set_default_fm(false);
        core.borrow_mut().config.settings.default_file_manager = false;
        core.borrow().config.save();
        state.set_status_text("默认文件管理器已由其它程序接管，已关闭本应用开关".into());
    } else if let Some(error) = default_fm_repair_error {
        state.set_status_text(format!("默认文件管理器自愈失败: {}", error).into());
    }

    // 启动时把持久化的栏宽/列宽推送到 AppState
    {
        let lay = &core.borrow().config.layout;
        state.set_sidebar_w(lay.sidebar_w);
        state.set_details_w(lay.details_w);
        state.set_col_modified_w(lay.col_modified);
        state.set_col_kind_w(lay.col_kind);
        state.set_col_size_w(lay.col_size);
        state.set_dual_ratio(lay.dual_ratio.clamp(0.15, 0.85));
    }

    // 恢复上次关闭时的窗口位置与大小（物理像素；win_w<=0 表示首启，用默认值）
    {
        let (x, y, w, h, maximized) = {
            let lay = &core.borrow().config.layout;
            (
                lay.win_x,
                lay.win_y,
                lay.win_w,
                lay.win_h,
                lay.win_maximized,
            )
        };
        if w > 200 && h > 200 {
            ui.window()
                .set_size(slint::PhysicalSize::new(w as u32, h as u32));
            ui.window().set_position(slint::PhysicalPosition::new(x, y));
        }
        if maximized {
            ui.window().set_maximized(true);
            ui.set_window_maximized(true);
        }
    }

    // 启动时推送已保存的网络位置列表（设置「云存储账号」页展示）
    ui_bridge::push_network_locations(&ui, &core.borrow());

    // —— 绑定全部回调 ——
    bind_navigation(&ui, &core);
    bind_selection(&ui, &core);
    bind_operations(&ui, &core);
    bind_new_menu(&ui, &core);
    bind_context_menu_ext(&ui, &core);
    bind_view_and_search(&ui, &core);
    bind_hash(&ui, &core);
    bind_tabs(&ui, &core);
    bind_window_chrome(&ui, &core);
    bind_layout(&ui, &core);
    bind_settings(&ui, &core);
    bind_right_pane(&ui, &core);

    // 当前目录实时监听：外部程序改动目录内容时自动软刷新（保留搜索与选中项）
    bind_watcher(&ui, &core);

    // 同名冲突对话框：复制 / 移动遇到目标已存在同名项时询问用户处置方式
    bind_conflict(&ui, &core);

    // 命令面板（Ctrl+P）：Rust 过滤命令 + 路径跳转
    bind_command_palette(&ui, &core);

    // 设备/驱动器热插拔定时轮询：插拔 U 盘/手机或挂载/卸载分区时自动刷新侧边栏与此电脑视图
    bind_device_polling(&ui, &core);

    // 应用更新：关于页 GitHub 链接 + 检查更新 / 带进度下载 / 启动安装
    bind_update(&ui, &core);

    // 文件名索引：重建（带进度）+ 后台索引开关的启动自动重建
    bind_index(&ui, &core);

    ui.run()
}

/// 计算"设备 + 驱动器"拓扑签名：用于轮询时判断是否发生插拔/挂载变化。
/// 仅纳入设备虚拟路径集合与驱动器盘符/总容量（不含可用空间，避免写盘时频繁误刷）。
fn device_topology_signature() -> String {
    let mut sig = String::new();
    for dev in fs::devices::list_devices() {
        sig.push_str(&dev.path);
        sig.push('\u{1}');
    }
    sig.push('|');
    for d in fs::disk::list_disks() {
        sig.push_str(&d.letter);
        sig.push(':');
        sig.push_str(&d.total.to_string());
        sig.push(';');
    }
    // 回收站空/满状态：变化时侧边栏图标需跟随切换（空→满 / 清空）
    sig.push('|');
    sig.push(if fs::recyclebin::is_empty().unwrap_or(true) {
        '0'
    } else {
        '1'
    });
    sig
}

/// 启动 ~1.5s 周期定时器轮询设备/驱动器拓扑；仅在签名变化时刷新，避免无谓重绘。
fn bind_device_polling(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let timer = Box::leak(Box::new(slint::Timer::default()));
    let w = ui.as_weak();
    let c = core.clone();
    // 初始签名：以启动时拓扑为基线，首次变化才触发刷新
    let last_sig = Rc::new(RefCell::new(device_topology_signature()));
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(1500),
        move || {
            let Some(ui) = w.upgrade() else { return };
            let sig = device_topology_signature();
            if *last_sig.borrow() == sig {
                return;
            }
            *last_sig.borrow_mut() = sig;

            // 重建侧边栏（当前活动标签路径用于高亮）
            let path = c.borrow().active_tab().history.current().clone();
            {
                let cc = c.borrow();
                ui.global::<AppState>()
                    .set_nav_items(ui_bridge::build_sidebar(
                        &path,
                        &cc.collapsed_sections,
                        &cc.config,
                    ));
            }
            // 若当前正浏览"此电脑"，重新加载条目以反映新增/移除的驱动器与设备
            if path.to_string_lossy() == fs::virtualfs::THIS_PC_PATH {
                load_current(&ui, &c);
            }
            // 右面板若也在"此电脑"，同步刷新
            let r_at_this_pc = c.borrow().right_pane.history.current().to_string_lossy()
                == fs::virtualfs::THIS_PC_PATH;
            if r_at_this_pc {
                load_right(&ui, &c);
            }
        },
    );
}

/// 绑定应用更新：关于页的 GitHub 链接、检查更新、带进度下载与安装启动。
/// 网络请求在后台线程执行（ureq 阻塞式），进度经 `invoke_from_event_loop`
/// 回主线程刷新 —— 与后台文件任务同一模式。
fn bind_update(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let state = ui.global::<AppState>();
    // 版本号与仓库地址推送到关于页
    state.set_app_version(update::CURRENT_VERSION.into());
    state.set_repo_url(update::REPO_URL.into());

    // 用系统默认浏览器打开链接（GitHub 仓库 / Issues）
    state.on_open_url(|url| {
        let _ = open::that(url.as_str());
    });

    // 检查结果与下载产物：用 Arc<Mutex> 存放——工作线程回填结果的
    // invoke_from_event_loop 闭包要求 Send，Rc<RefCell> 无法跨线程捕获
    let latest: Arc<Mutex<Option<update::ReleaseInfo>>> = Arc::new(Mutex::new(None));
    let installer: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let cancel = Arc::new(AtomicBool::new(false));

    // —— 检查更新 ——
    {
        let w = ui.as_weak();
        let latest = latest.clone();
        state.on_check_update(move || {
            if let Some(ui) = w.upgrade() {
                ui.global::<AppState>().set_update_state(1); // 检查中
            }
            let w = w.clone();
            let latest = latest.clone();
            std::thread::spawn(move || {
                let result = update::check_latest();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let st = ui.global::<AppState>();
                    match result {
                        Ok(info) => {
                            if update::is_newer(&info.version, update::CURRENT_VERSION) {
                                st.set_update_latest_version(info.version.as_str().into());
                                st.set_update_notes(info.notes.as_str().into());
                                st.set_update_state(3); // 发现新版
                                *latest.lock().unwrap() = Some(info);
                            } else {
                                st.set_update_state(2); // 已是最新
                            }
                        }
                        Err(e) => {
                            st.set_update_error(e.into());
                            st.set_update_state(6); // 出错
                        }
                    }
                });
            });
        });
    }

    // —— 下载更新（带进度）——
    {
        let w = ui.as_weak();
        let latest = latest.clone();
        let installer = installer.clone();
        let cancel = cancel.clone();
        state.on_download_update(move || {
            let Some(info) = latest.lock().unwrap().clone() else {
                return;
            };
            cancel.store(false, Ordering::Relaxed);
            if let Some(ui) = w.upgrade() {
                let st = ui.global::<AppState>();
                st.set_update_progress(0.0);
                st.set_update_progress_text("准备下载…".into());
                st.set_update_state(4); // 下载中
            }
            let w = w.clone();
            let installer = installer.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                let w_prog = w.clone();
                let result = update::download(&info, &cancel, move |done, total, speed| {
                    let frac = if total > 0 {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let text = format!(
                        "{} / {} · {}/s",
                        fs::metadata::human_size(done),
                        fs::metadata::human_size(total),
                        fs::metadata::human_size(speed as u64),
                    );
                    let w = w_prog.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = w.upgrade() {
                            let st = ui.global::<AppState>();
                            st.set_update_progress(frac);
                            st.set_update_progress_text(text.into());
                        }
                    });
                });
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = w.upgrade() else { return };
                    let st = ui.global::<AppState>();
                    match result {
                        Ok(path) => {
                            *installer.lock().unwrap() = Some(path);
                            st.set_update_state(5); // 下载完成待安装
                        }
                        // 用户主动取消：回到「发现新版」可重新下载
                        Err(e) if e == "已取消" => st.set_update_state(3),
                        Err(e) => {
                            st.set_update_error(e.into());
                            st.set_update_state(6);
                        }
                    }
                });
            });
        });
    }

    // —— 取消下载 ——
    {
        let cancel = cancel.clone();
        state.on_cancel_download(move || {
            cancel.store(true, Ordering::Relaxed);
        });
    }

    // —— 安装并重启：启动安装程序（分离进程）后退出应用，避免 exe 被占用 ——
    {
        let w = ui.as_weak();
        let c = core.clone();
        state.on_install_update(move || {
            let path = installer.lock().unwrap().clone();
            let Some(path) = path else { return };
            match update::run_installer(&path) {
                Ok(()) => {
                    // 退出前保存窗口几何，安装重启后可恢复位置
                    if let Some(ui) = w.upgrade() {
                        save_window_geometry(&ui, &c);
                    }
                    let _ = slint::quit_event_loop();
                }
                Err(e) => {
                    if let Some(ui) = w.upgrade() {
                        let st = ui.global::<AppState>();
                        st.set_update_error(e.into());
                        st.set_update_state(6);
                    }
                }
            }
        });
    }
}

// ─── 文件名索引 ───

/// 绑定索引重建：设置页「重建索引」按钮 + 后台索引开关的启动自动重建
fn bind_index(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();
    // 启动时推送已有索引的概况（惰性加载磁盘索引文件）
    {
        let info = fs::index::summary();
        if !info.is_empty() {
            state.set_index_info(info.into());
        }
    }

    let w = ui.as_weak();
    let c = core.clone();
    state.on_rebuild_index(move || {
        if let Some(ui) = w.upgrade() {
            start_index_rebuild(&ui, &c);
        }
    });

    // 后台索引开启且尚无索引文件：启动时静默自动重建
    let auto = {
        let cc = core.borrow();
        cc.config.settings.background_index && !fs::index::exists()
    };
    if auto {
        start_index_rebuild(ui, core);
    }
}

/// 启动一次后台索引重建（已在重建中则忽略），进度回填到设置页进度条
fn start_index_rebuild(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    if fs::index::is_rebuilding() {
        return;
    }
    let scope = core.borrow().config.settings.index_location.clone();
    let st = ui.global::<AppState>();
    st.set_index_state(1);
    st.set_index_progress(0.0);
    st.set_index_progress_text("正在枚举目录…".into());

    let w = ui.as_weak();
    std::thread::spawn(move || {
        let w_prog = w.clone();
        let result = fs::index::rebuild(&scope, move |frac, count, current| {
            let text = if current.is_empty() {
                format!("已索引 {} 项", count)
            } else {
                format!("已索引 {} 项 · {}", count, current)
            };
            let w = w_prog.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = w.upgrade() {
                    let st = ui.global::<AppState>();
                    st.set_index_progress(frac);
                    st.set_index_progress_text(text.into());
                }
            });
        });
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = w.upgrade() else { return };
            let st = ui.global::<AppState>();
            st.set_index_state(0);
            match result {
                Ok(count) => {
                    st.set_index_info(format!("索引就绪：共 {} 项，深层搜索可用", count).into())
                }
                Err(e) => st.set_index_info(format!("重建失败：{}", e).into()),
            }
        });
    });
}

/// 绑定布局持久化：拖拽分隔条结束后把栏宽/列宽写回配置文件
fn bind_layout(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();
    let c = core.clone();
    state.on_save_layout(move |sidebar, details, col_mod, col_kind, col_size| {
        let mut core = c.borrow_mut();
        let lay = &mut core.config.layout;
        lay.sidebar_w = sidebar;
        lay.details_w = details;
        lay.col_modified = col_mod;
        lay.col_kind = col_kind;
        lay.col_size = col_size;
        core.config.save();
    });

    // 切换条目标签：打标签 / 取消，持久化后刷新（计数 + 当前标签视图）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_toggle_tag(move |idx, key| {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let path = {
                let core = c.borrow();
                core.pane_entry_at(right, idx as usize)
                    .map(|e| e.path.clone())
            };
            if let Some(path) = path {
                {
                    let mut core = c.borrow_mut();
                    core.config.toggle_tag(&path, key.as_str());
                    core.config.save();
                }
                // 重新加载活动面板（刷新标签角标 / 若在标签视图则更新列表）+ 侧边栏计数
                reload_active_pane(&ui, &c);
            }
        }
    });

    // 顶部工具栏「标记」下拉：对全部选中项统一切换某标签。
    // 语义：若选中项全部已含该标签 → 全部去除；否则 → 全部添加。
    let w = ui.as_weak();
    let c = core.clone();
    state.on_tag_selected(move |key| {
        if let Some(ui) = w.upgrade() {
            let key = key.to_string();
            // 按活动面板取选中项（双面板右侧活动时作用于右面板）
            let right = toolbar_routes_right(&ui);
            let paths: Vec<String> = {
                let core = c.borrow();
                core.pane_selected_paths(right)
                    .iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect()
            };
            if paths.is_empty() {
                return;
            }
            {
                let mut core = c.borrow_mut();
                // 目标状态：仅当全部已含该标签时才去除，否则全部添加
                let all_tagged = paths.iter().all(|p| core.config.has_tag(p, &key));
                let target = !all_tagged;
                for p in &paths {
                    if core.config.has_tag(p, &key) != target {
                        core.config.toggle_tag(p, &key);
                    }
                }
                core.config.save();
            }
            reload_active_pane(&ui, &c);
            // 刷新「标记」下拉的对勾状态
            ui_bridge::update_selection_pane(&ui, &c.borrow(), right);
        }
    });

    // 回收站还原：把 $R 移回原位置
    let w = ui.as_weak();
    let c = core.clone();
    state.on_restore_item(move |idx| {
        if let Some(ui) = w.upgrade() {
            let r_path = {
                let core = c.borrow();
                core.entry_at(idx as usize).map(|e| e.path.clone())
            };
            if let Some(r_path) = r_path {
                match fs::recyclebin::restore(&r_path) {
                    Ok(_) => {
                        load_current(&ui, &c);
                        ui.global::<AppState>()
                            .set_status_text("已还原到原位置".into());
                    }
                    Err(e) => {
                        ui.global::<AppState>()
                            .set_status_text(format!("还原失败：{}", e).into());
                    }
                }
            }
        }
    });

    // 回收站：恢复选中项
    let w = ui.as_weak();
    let c = core.clone();
    state.on_restore_selected(move || {
        if let Some(ui) = w.upgrade() {
            let paths = c.borrow().selected_paths();
            let (mut ok, mut fail) = (0, 0);
            for p in &paths {
                match fs::recyclebin::restore(&p.to_string_lossy()) {
                    Ok(_) => ok += 1,
                    Err(_) => fail += 1,
                }
            }
            load_current(&ui, &c);
            ui.global::<AppState>()
                .set_status_text(format!("已恢复 {} 项，失败 {} 项", ok, fail).into());
        }
    });

    // 回收站：恢复全部项
    let w = ui.as_weak();
    let c = core.clone();
    state.on_restore_all(move || {
        if let Some(ui) = w.upgrade() {
            let paths: Vec<String> = {
                let core = c.borrow();
                core.active_tab()
                    .entries
                    .iter()
                    .map(|e| e.path.clone())
                    .collect()
            };
            let (mut ok, mut fail) = (0, 0);
            for p in &paths {
                match fs::recyclebin::restore(p) {
                    Ok(_) => ok += 1,
                    Err(_) => fail += 1,
                }
            }
            load_current(&ui, &c);
            ui.global::<AppState>()
                .set_status_text(format!("已恢复全部：{} 项，失败 {} 项", ok, fail).into());
        }
    });

    // 回收站：彻底删除选中项（不可逆）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_delete_permanent_selected(move || {
        if let Some(ui) = w.upgrade() {
            let paths = c.borrow().selected_paths();
            let mut ok = 0;
            for p in &paths {
                if fs::recyclebin::delete_permanent(&p.to_string_lossy()).is_ok() {
                    ok += 1;
                }
            }
            load_current(&ui, &c);
            ui.global::<AppState>()
                .set_status_text(format!("已彻底删除 {} 项", ok).into());
        }
    });

    // 回收站：彻底删除全部（清空回收站，不可逆）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_delete_permanent_all(move || {
        if let Some(ui) = w.upgrade() {
            let paths: Vec<String> = {
                let core = c.borrow();
                core.active_tab()
                    .entries
                    .iter()
                    .map(|e| e.path.clone())
                    .collect()
            };
            let mut ok = 0;
            for p in &paths {
                if fs::recyclebin::delete_permanent(p).is_ok() {
                    ok += 1;
                }
            }
            load_current(&ui, &c);
            ui.global::<AppState>()
                .set_status_text(format!("回收站已清空：彻底删除 {} 项", ok).into());
        }
    });
}

/// 读取当前活跃标签页目录并推送到 UI
fn load_current(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    // 导航/刷新时清除可能残留的跨面板拖拽幽灵状态：
    // 拖拽启动后若因目录切换导致 InputOverlay 重建、pointer-up 丢失，幽灵会卡住。
    ui.global::<AppState>().set_pane_drag_active(false);
    // 同时退出行内重命名（两侧下标一并清）：editing 下标残留会禁用 InputOverlay，
    // 表现为「编辑框一直显示且界面无法点击」（如在新面板打开后旧编辑态未清）。
    ui.invoke_clear_editing();
    // 设置标签页不读取文件系统，仅推送标签与视图状态
    let is_settings = core.borrow().active_tab().kind == app::TabKind::Settings;
    if is_settings {
        {
            let c = core.borrow();
            ui_bridge::push_entries(ui, &c);
            ui_bridge::push_tabs(ui, &c);
        }
        // 设置页无目录内容，停止实时监听
        if let Some(w) = core.borrow_mut().watcher.as_mut() {
            w.clear();
        }
        return;
    }

    let path = core.borrow().active_tab().history.current().clone();
    let path_str = path.to_string_lossy().to_string();

    // 虚拟路径（标签 / 回收站 / 网络位置）走 provider 生成条目
    if fs::virtualfs::is_virtual(&path_str) {
        let entries = {
            let mut c = core.borrow_mut();
            fs::virtualfs::resolve(&path_str, &mut c.config).unwrap_or_default()
        };
        {
            let mut c = core.borrow_mut();
            let folders_first = c.config.settings.folders_first;
            let tab = c.active_tab_mut();
            tab.entries = entries;
            tab.folders_first = folders_first;
            tab.search.clear();
            tab.rebuild();
        }
        {
            let c = core.borrow();
            ui_bridge::push_entries(ui, &c);
            ui_bridge::push_tabs(ui, &c);
            ui.global::<AppState>()
                .set_nav_items(ui_bridge::build_sidebar(
                    &path,
                    &c.collapsed_sections,
                    &c.config,
                ));
        }
        // 虚拟路径（标签 / 回收站 / 网络位置）内容不由文件系统驱动，停止实时监听
        if let Some(w) = core.borrow_mut().watcher.as_mut() {
            w.clear();
        }
        return;
    }

    // 提前取出设置项，避免 match 表达式中的临时借用与内部 borrow_mut 冲突
    let (show_hidden, folders_first) = {
        let c = core.borrow();
        (
            c.config.settings.show_hidden,
            c.config.settings.folders_first,
        )
    };
    match ops::read_dir(&path, show_hidden) {
        Ok(entries) => {
            let mut c = core.borrow_mut();
            let tab = c.active_tab_mut();
            tab.entries = entries;
            tab.folders_first = folders_first;
            tab.search.clear();
            tab.rebuild();
        }
        Err(e) => {
            let st = ui.global::<AppState>();
            st.set_status_text(format!("无法打开目录：{}", e).into());
            return;
        }
    }
    {
        let c = core.borrow();
        ui_bridge::push_entries(ui, &c);
        ui_bridge::push_tabs(ui, &c);
        ui.global::<AppState>()
            .set_nav_items(ui_bridge::build_sidebar(
                &path,
                &c.collapsed_sections,
                &c.config,
            ));
    }
    // 更新实时监听到新的当前目录（notify 后端非递归监听其直接子项变化）
    if let Some(w) = core.borrow_mut().watcher.as_mut() {
        w.watch(&path);
    }
}

/// 目录实时监听触发的「软刷新」：重读当前活跃标签目录并推送 UI，
/// 与 `load_current` 不同的是——保留当前搜索词，并按路径尽量恢复刷新前的选中项，
/// 避免外部程序（或后台任务）改动目录时打断用户正在进行的浏览 / 多选。
/// 仅处理普通文件标签页的真实目录；设置页与虚拟路径直接忽略。
fn reload_current_soft(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    if core.borrow().active_tab().kind != app::TabKind::Files {
        return;
    }
    let path = core.borrow().active_tab().history.current().clone();
    let path_str = path.to_string_lossy().to_string();
    if fs::virtualfs::is_virtual(&path_str) {
        return;
    }

    let (show_hidden, folders_first) = {
        let c = core.borrow();
        (
            c.config.settings.show_hidden,
            c.config.settings.folders_first,
        )
    };
    let entries = match ops::read_dir(&path, show_hidden) {
        Ok(e) => e,
        Err(_) => return, // 目录已被删除/移动等：留待用户主动导航，不打断当前视图
    };

    {
        let mut c = core.borrow_mut();
        // 记录刷新前的选中项路径，用于按路径恢复
        let prev: std::collections::HashSet<String> = c
            .active_tab()
            .selected_paths()
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let tab = c.active_tab_mut();
        tab.entries = entries;
        tab.folders_first = folders_first;
        // 注意：不清空 tab.search，rebuild 会按现有搜索词重建 filtered
        tab.rebuild();
        // 按路径恢复选中（条目可能已重排，先收集下标再置位以避开借用冲突）
        if !prev.is_empty() {
            let to_select: Vec<usize> = (0..tab.filtered.len())
                .filter(|&fi| tab.entry_at(fi).is_some_and(|e| prev.contains(&e.path)))
                .collect();
            for fi in to_select {
                tab.selected[fi] = true;
            }
        }
    }

    let c = core.borrow();
    ui_bridge::push_entries(ui, &c);
    ui_bridge::push_tabs(ui, &c);
}

/// 初始化当前目录实时监听：绑定 `auto-refresh` 回调到软刷新，创建 `DirWatcher`
/// 并注入 `AppCore`，随后立即监听启动目录。监听后端不可用时静默降级为手动 F5。
fn bind_watcher(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 监听线程经事件循环回主线程后，在此执行软刷新
    let w = ui.as_weak();
    let c = core.clone();
    state.on_auto_refresh(move || {
        if let Some(ui) = w.upgrade() {
            reload_current_soft(&ui, &c);
        }
    });

    // notify 事件（已防抖）→ 回主线程触发 auto-refresh 回调
    let w = ui.as_weak();
    let watcher = fs::watcher::DirWatcher::new(move || {
        let w = w.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w.upgrade() {
                ui.global::<AppState>().invoke_auto_refresh();
            }
        });
    });

    if let Ok(mut watcher) = watcher {
        // 立即监听启动目录（虚拟路径不监听）
        let cur = core.borrow().active_tab().history.current().clone();
        if !fs::virtualfs::is_virtual(&cur.to_string_lossy()) {
            watcher.watch(&cur);
        }
        core.borrow_mut().watcher = Some(watcher);
    }
}

/// 绑定同名冲突对话框的处置回调：用户选择后关闭对话框，并把决策经桥回送给
/// 正在阻塞等待的后台工作线程。
fn bind_conflict(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();
    let w = ui.as_weak();
    let bridge = core.borrow().conflict_bridge.clone();
    state.on_conflict_choose(move |decision, apply_all| {
        if let Some(ui) = w.upgrade() {
            ui.global::<AppState>().set_conflict_open(false);
        }
        let decision = match decision.as_str() {
            "overwrite" => fs::tasks::ConflictDecision::Overwrite,
            "rename" => fs::tasks::ConflictDecision::Rename,
            _ => fs::tasks::ConflictDecision::Skip,
        };
        if let Some(tx) = bridge.pending.lock().unwrap().take() {
            let _ = tx.send(fs::tasks::ConflictReply {
                decision,
                apply_all,
            });
        }
    });
}

/// 跨目录移动单个路径（同盘 rename；自动创建目标父目录）。
fn move_path(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::rename(from, to)
}

/// 执行撤销（逆操作），返回状态栏消息。
fn apply_undo(action: &app::UndoAction) -> String {
    use app::UndoAction::*;
    match action {
        Rename { orig, renamed } => {
            let name = orig
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match ops::rename(renamed, &name) {
                Ok(_) => format!("已撤销重命名（改回 {}）", name),
                Err(e) => format!("撤销失败：{}", e),
            }
        }
        Create { path } => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match fs::recyclebin::move_to_recycle_bin(&[path.clone()]) {
                Ok(_) => format!("已撤销新建（{} 移入回收站）", name),
                Err(e) => format!("撤销失败：{}", e),
            }
        }
        Move { pairs } => {
            let ok = pairs
                .iter()
                .filter(|(src, dst)| move_path(dst, src).is_ok())
                .count();
            format!("已撤销移动（移回 {} 项）", ok)
        }
        Delete { paths } => {
            let ok = paths
                .iter()
                .filter(|p| fs::recyclebin::restore_to_original(p).is_ok())
                .count();
            format!("已撤销删除（还原 {} 项）", ok)
        }
    }
}

/// 执行重做（正向操作），返回状态栏消息。
fn apply_redo(action: &app::UndoAction) -> String {
    use app::UndoAction::*;
    match action {
        Rename { orig, renamed } => {
            let name = renamed
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match ops::rename(orig, &name) {
                Ok(_) => format!("已重做重命名（{}）", name),
                Err(e) => format!("重做失败：{}", e),
            }
        }
        Create { path } => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match fs::recyclebin::restore_to_original(path) {
                Ok(_) => format!("已重做新建（{} 从回收站还原）", name),
                Err(e) => format!("重做失败：{}", e),
            }
        }
        Move { pairs } => {
            let ok = pairs
                .iter()
                .filter(|(src, dst)| move_path(src, dst).is_ok())
                .count();
            format!("已重做移动（{} 项）", ok)
        }
        Delete { paths } => match fs::recyclebin::move_to_recycle_bin(paths) {
            Ok(_) => format!("已重做删除（{} 项移入回收站）", paths.len()),
            Err(e) => format!("重做失败：{}", e),
        },
    }
}

/// 命令面板可用命令表：(图标, 标题, 描述, 快捷键, action)
fn palette_all_commands() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    vec![
        (
            "\u{E80A}",
            "切换到网格视图",
            "以图标网格显示文件",
            "",
            "view-grid",
        ),
        (
            "\u{EA37}",
            "切换到详细信息视图",
            "显示名称、日期、类型、大小列",
            "",
            "view-list",
        ),
        (
            "\u{F0E2}",
            "切换双面板",
            "开启/关闭左右两个独立文件面板",
            "F3",
            "toggle-dual",
        ),
        (
            "\u{E90D}",
            "切换详情面板",
            "显示/隐藏右侧详情面板",
            "",
            "toggle-details",
        ),
        (
            "\u{E72C}",
            "刷新当前目录",
            "重新读取文件列表",
            "F5",
            "refresh",
        ),
        (
            "\u{E74A}",
            "转到上一级目录",
            "返回父文件夹",
            "Backspace",
            "go-up",
        ),
        ("\u{E72B}", "后退", "导航历史后退", "", "go-back"),
        ("\u{E72A}", "前进", "导航历史前进", "", "go-forward"),
        (
            "\u{E8F4}",
            "新建文件夹",
            "在当前目录创建文件夹",
            "Ctrl+Shift+N",
            "new-folder",
        ),
        (
            "\u{E8A5}",
            "新建文件",
            "在当前目录创建文本文件",
            "",
            "new-file",
        ),
        ("\u{E713}", "打开设置", "打开应用设置页", "", "settings"),
        (
            "\u{E7B3}",
            "切换隐藏文件",
            "显示或隐藏隐藏项",
            "",
            "toggle-hidden",
        ),
        (
            "\u{E712}",
            "添加网络位置",
            "挂载 SMB 共享到盘符并浏览",
            "",
            "add-network-location",
        ),
    ]
}

/// Omnibar 路径补全：解析输入路径的父目录，列出以前缀开头的子项。
/// 输入以分隔符结尾时前缀为空（列全部）；父目录不可用时回退到活动面板当前目录。
fn compute_path_completions(input: &str, base: &Path) -> Vec<Crumb> {
    let p = Path::new(input);
    let (dir, prefix) = if input.ends_with('/') || input.ends_with('\\') {
        (p.to_path_buf(), String::new())
    } else {
        match p.parent() {
            Some(par) if !par.as_os_str().is_empty() => {
                let pre = p
                    .file_name()
                    .map(|f| f.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                (par.to_path_buf(), pre)
            }
            _ => (base.to_path_buf(), input.to_lowercase()),
        }
    };
    let mut comps: Vec<Crumb> = Vec::new();
    if dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if name.to_lowercase().starts_with(&prefix) {
                    comps.push(Crumb {
                        name: name.into(),
                        path: ent.path().to_string_lossy().to_string().into(),
                    });
                }
            }
        }
    }
    comps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    comps.truncate(50);
    comps
}

/// 绑定命令面板：Rust 侧过滤命令 + 路径识别 + 执行分发（Ctrl+P 触发）。
fn bind_command_palette(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 查询：按标题/描述模糊过滤；输入为存在目录时置顶「跳转」命令
    let w = ui.as_weak();
    state.on_palette_query(move |query| {
        if let Some(ui) = w.upgrade() {
            let qt = query.trim().to_string();
            let ql = qt.to_lowercase();
            let mut rows: Vec<Command> = Vec::new();

            if !qt.is_empty() && Path::new(&qt).is_dir() {
                rows.push(Command {
                    icon: "\u{E8DA}".into(),
                    title: format!("跳转到 {}", qt).into(),
                    description: "打开该目录".into(),
                    shortcut: "Enter".into(),
                    action: format!("cd:{}", qt).into(),
                });
            }

            for (icon, title, desc, sc, action) in palette_all_commands() {
                if ql.is_empty()
                    || title.to_lowercase().contains(&ql)
                    || desc.to_lowercase().contains(&ql)
                {
                    rows.push(Command {
                        icon: icon.into(),
                        title: title.into(),
                        description: desc.into(),
                        shortcut: sc.into(),
                        action: action.into(),
                    });
                }
            }

            let st = ui.global::<AppState>();
            st.set_palette_commands(slint::ModelRc::new(slint::VecModel::from(rows)));
            st.set_palette_selected(0);
        }
    });

    // 执行：cd: 前缀跳转目录，其余分发到对应 AppState 回调
    let w = ui.as_weak();
    let c = core.clone();
    state.on_palette_run(move |action| {
        if let Some(ui) = w.upgrade() {
            let st = ui.global::<AppState>();
            st.set_command_palette_open(false);
            let a = action.to_string();
            if let Some(path) = a.strip_prefix("cd:") {
                navigate_to(&ui, &c, PathBuf::from(path));
                return;
            }
            match a.as_str() {
                "view-grid" => st.invoke_set_view("grid".into()),
                "view-list" => st.invoke_set_view("list".into()),
                "toggle-dual" => st.invoke_toggle_dual(),
                "toggle-details" => st.invoke_toggle_details(),
                "refresh" => st.invoke_refresh(),
                "go-up" => st.invoke_go_up(),
                "go-back" => st.invoke_go_back(),
                "go-forward" => st.invoke_go_forward(),
                "new-folder" => st.invoke_new_folder(),
                "new-file" => st.invoke_new_file(),
                "settings" => st.invoke_open_settings_tab(),
                "toggle-hidden" => {
                    let cur = st.get_set_show_hidden();
                    st.invoke_set_bool("show-hidden".into(), !cur);
                }
                "add-network-location" => {
                    st.set_netloc_dialog_open(true);
                }
                _ => {}
            }
        }
    });
}

/// 从队列取出下一个任务并在工作线程执行；空闲时才启动，进度经事件循环回填 UI。
fn start_next_job(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    use std::sync::Arc;

    // 已有任务在跑则等其完成时再串联；否则取队首
    let job = {
        let mut c = core.borrow_mut();
        if c.task_control.is_some() {
            return;
        }
        match c.task_queue.pop_front() {
            Some(j) => j,
            None => return,
        }
    };

    let ctrl = Arc::new(fs::tasks::TaskControl::new());
    core.borrow_mut().task_control = Some(ctrl.clone());

    // 初始化进度卡片，避免首帧前的空白闪烁
    let op_label = job.kind.label();
    let dst_str = job.dst.to_string_lossy().to_string();
    let st = ui.global::<AppState>();
    st.set_task_active(true);
    st.set_task_paused(false);
    st.set_task_operation(op_label.into());
    st.set_task_current_file("准备中…".into());
    st.set_task_target(dst_str.into());
    st.set_task_completed(0);
    st.set_task_total(0);
    st.set_task_progress(0.0);
    st.set_task_speed("计算中…".into());
    st.set_task_eta("计算中…".into());

    let w_progress = ui.as_weak();
    let w_done = ui.as_weak();
    let w_ask = ui.as_weak();
    let bridge = core.borrow().conflict_bridge.clone();
    std::thread::spawn(move || {
        let result = fs::tasks::run(
            job,
            ctrl,
            move |p| {
                let w = w_progress.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        let st = ui.global::<AppState>();
                        st.set_task_operation(p.operation.into());
                        st.set_task_current_file(p.current_file.into());
                        st.set_task_target(p.target.into());
                        st.set_task_completed(p.completed);
                        st.set_task_total(p.total);
                        st.set_task_progress(p.fraction);
                        st.set_task_speed(p.speed.into());
                        st.set_task_eta(p.eta.into());
                    }
                });
            },
            move |q| {
                // 遇顶层同名冲突：把回复通道存入桥，请主线程弹窗，随后阻塞等待用户选择
                let (tx, rx) = std::sync::mpsc::channel();
                *bridge.pending.lock().unwrap() = Some(tx);
                let w = w_ask.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = w.upgrade() {
                        let st = ui.global::<AppState>();
                        st.set_conflict_name(q.name.into());
                        st.set_conflict_operation(q.operation.into());
                        st.set_conflict_src_info(q.src_info.into());
                        st.set_conflict_dst_info(q.dst_info.into());
                        st.set_conflict_is_dir(q.is_dir);
                        st.set_conflict_apply_all(false);
                        st.set_conflict_open(true);
                    }
                });
                // 对话框被异常关闭 / 事件循环失效时默认跳过，保证工作线程不会永久阻塞
                rx.recv().unwrap_or(fs::tasks::ConflictReply {
                    decision: fs::tasks::ConflictDecision::Skip,
                    apply_all: false,
                })
            },
        );

        // 完成：回主线程触发 task-finished（在那里访问 core 重载目录、串联下一项）
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = w_done.upgrade() {
                let skip_note = if result.skipped > 0 {
                    format!("，跳过 {} 项", result.skipped)
                } else {
                    String::new()
                };
                let msg = if result.cancelled {
                    format!("已取消（完成 {} 项{}）", result.ok, skip_note)
                } else if result.error.is_empty() {
                    format!("已完成 {} 个项目{}", result.ok, skip_note)
                } else {
                    format!("操作部分失败：{}", result.error)
                };
                ui.global::<AppState>()
                    .invoke_task_finished(result.ok, msg.into());
            }
        });
    });
}

/// 把「压缩选中项为归档」入队为后台任务（进度卡片显示速度/ETA，可暂停/取消）。
/// `fmt` 为输出格式："zip" / "7z" / "tar" / "targz"；
/// `idx` 为无选中时的回退目标项（-1 表示无）。
fn enqueue_compress(ui: &MainWindow, c: &Rc<RefCell<AppCore>>, idx: i32, fmt: &str) {
    let right = toolbar_routes_right(ui);
    let (items, dst_dir) = {
        let core = c.borrow();
        let mut paths = core.pane_selected_paths(right);
        if paths.is_empty() {
            if let Some(e) = core.pane_entry_at(right, idx as usize) {
                paths.push(PathBuf::from(&e.path));
            }
        }
        (paths, core.pane(right).history.current().clone())
    };
    if items.is_empty() {
        return;
    }
    let ext = match fmt {
        "7z" => "7z",
        "tar" => "tar",
        "targz" => "tar.gz",
        _ => "zip",
    };
    // 输出路径入队时即确定（重名自动避让），由后台任务流式写入。
    // 立即占位创建空文件：任务串行执行，若不占位，排队中的同名压缩任务
    // 在入队时看不到前一任务的产物，会解析到相同路径并互相覆盖/误删。
    let target =
        ops::resolve_conflict(dst_dir.join(format!("{}.{}", ops::archive_stem(&items), ext)));
    let _ = std::fs::File::create(&target);
    c.borrow_mut().task_queue.push_back(fs::tasks::Job {
        kind: fs::tasks::TaskKind::Compress,
        srcs: items,
        dst: target,
    });
    start_next_job(ui, c);
}

/// 在指定面板执行行内重命名提交并刷新该面板（记录撤销）
fn rename_in_pane(
    ui: &MainWindow,
    c: &Rc<RefCell<AppCore>>,
    right: bool,
    idx: i32,
    new_name: &str,
) {
    let old = c
        .borrow()
        .pane_entry_at(right, idx as usize)
        .map(|e| PathBuf::from(&e.path));
    if let Some(old) = old {
        if !new_name.trim().is_empty() {
            if let Ok(new_path) = ops::rename(&old, new_name) {
                // 记录撤销：orig=旧完整路径, renamed=新完整路径
                c.borrow_mut().record_undo(app::UndoAction::Rename {
                    orig: old.clone(),
                    renamed: new_path,
                });
            }
        }
    }
    ui.invoke_clear_editing();
    if right {
        load_right(ui, c);
    } else {
        load_current(ui, c);
    }
}

/// 收集活动面板选中的可解压归档与当前目录
fn selected_archives(ui: &MainWindow, c: &Rc<RefCell<AppCore>>) -> (Vec<PathBuf>, PathBuf) {
    let right = toolbar_routes_right(ui);
    let core = c.borrow();
    let archives: Vec<PathBuf> = core
        .pane_selected_paths(right)
        .into_iter()
        .filter(|p| ops::is_zip_archive(p))
        .collect();
    (archives, core.pane(right).history.current().clone())
}

/// 公用工具栏导航是否应路由到右侧面板（双面板开启且活动面板为右侧）
fn toolbar_routes_right(ui: &MainWindow) -> bool {
    let state = ui.global::<AppState>();
    state.get_dual_pane() && state.get_active_pane() == "right"
}

/// 按活动面板刷新：双面板右侧活动时刷新右面板，否则刷新当前活动标签。
fn reload_active_pane(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    if toolbar_routes_right(ui) {
        load_right(ui, core);
    } else {
        load_current(ui, core);
    }
}

/// 在当前活跃标签页跳转到指定路径（支持虚拟路径 tag:// recycle:// network://）
fn navigate_to(ui: &MainWindow, core: &Rc<RefCell<AppCore>>, target: PathBuf) {
    let target_str = target.to_string_lossy().to_string();
    if !fs::virtualfs::is_virtual(&target_str) && !target.is_dir() {
        return;
    }
    core.borrow_mut().active_tab_mut().history.navigate(target);
    load_current(ui, core);
}

// ─── 双面板：右侧独立面板 ───

/// 读取右侧面板当前目录并推送到 UI 的 r-* 属性
fn load_right(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    // 与 load_current 同理：清除可能残留的跨面板拖拽幽灵（如"在新面板中打开"后），
    // 并退出行内重命名——r-entries 重建后残留的 r-editing-index 会把编辑框
    // 错挂到新列表同下标条目上
    ui.global::<AppState>().set_pane_drag_active(false);
    ui.invoke_clear_editing();
    let path = core.borrow().right_pane.history.current().clone();
    let (show_hidden, folders_first) = {
        let c = core.borrow();
        (
            c.config.settings.show_hidden,
            c.config.settings.folders_first,
        )
    };
    match ops::read_dir(&path, show_hidden) {
        Ok(entries) => {
            let mut c = core.borrow_mut();
            let t = &mut c.right_pane;
            t.entries = entries;
            t.folders_first = folders_first;
            t.search.clear();
            t.rebuild();
        }
        Err(e) => {
            ui.global::<AppState>()
                .set_status_text(format!("右面板无法打开目录：{}", e).into());
            return;
        }
    }
    ui_bridge::push_right(ui, &core.borrow());
    // 导航/刷新已清空右面板选中：右面板为活动面板时同步 sel-* 全局状态
    // （否则 ActionBar 按钮可用性/详情栏残留导航前的选中信息）
    if toolbar_routes_right(ui) {
        ui_bridge::update_selection_pane(ui, &core.borrow(), true);
    }
}

/// 右侧面板跳转到指定目录（仅限真实目录）
fn navigate_right(ui: &MainWindow, core: &Rc<RefCell<AppCore>>, target: PathBuf) {
    if !target.is_dir() {
        return;
    }
    core.borrow_mut().right_pane.history.navigate(target);
    load_right(ui, core);
}

/// 绑定右侧面板的导航 / 选择 / 打开回调
fn bind_right_pane(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 切换活动面板（点击面板内容/空白时调用），并把 sel-* 全局选中信息
    // （详情栏 / 属性 / 解压按钮可见性）重新同步为新活动面板的选中项
    let w = ui.as_weak();
    let c = core.clone();
    state.on_set_active_pane(move |side| {
        if let Some(ui) = w.upgrade() {
            let right = side == "right";
            // 切换面板即结束任何进行中的行内重命名（点击另一面板 = 取消编辑，
            // 与资源管理器一致），避免编辑框跨面板切换残留
            ui.invoke_clear_editing();
            ui.global::<AppState>().set_active_pane(side);
            if ui.global::<AppState>().get_dual_pane() {
                ui_bridge::update_selection_pane(&ui, &c.borrow(), right);
            }
        }
    });

    // 地址栏提交 / 面包屑跳转
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_navigate(move |path| {
        if let Some(ui) = w.upgrade() {
            navigate_right(&ui, &c, PathBuf::from(path.as_str()));
        }
    });

    // 双击：文件夹进入、文件打开
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_open_entry(move |idx| {
        if let Some(ui) = w.upgrade() {
            let target = {
                let core = c.borrow();
                core.right_pane
                    .entry_at(idx as usize)
                    .map(|e| (e.is_dir, e.path.clone()))
            };
            if let Some((is_dir, path)) = target {
                if is_dir {
                    navigate_right(&ui, &c, PathBuf::from(path));
                } else {
                    open_with_cwd(&path);
                }
            }
        }
    });

    // 单击：单选高亮（就地更新模型，保持双击连续触发）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_select_entry(move |idx| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let t = &mut core.right_pane;
                let i = idx as usize;
                if i < t.selected.len() {
                    for s in t.selected.iter_mut() {
                        *s = false;
                    }
                    t.selected[i] = true;
                    t.last_clicked = Some(i);
                }
            }
            ui_bridge::refresh_right_selection(&ui, &c.borrow());
            // 同步 sel-* 全局选中信息（详情栏 / 解压按钮可见性等）
            ui_bridge::update_selection_pane(&ui, &c.borrow(), true);
        }
    });

    // 右面板框选（支持多选）：语义同 on_box_select，作用于 right_pane
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_box_select(move |r0, r1, c0, c1, cols, additive| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let t = &mut core.right_pane;
                let n = t.selected.len() as i32;
                if !additive {
                    for s in t.selected.iter_mut() {
                        *s = false;
                    }
                }
                let cols = cols.max(1);
                let (lo_r, hi_r) = (r0.min(r1).max(0), r0.max(r1));
                let (lo_c, hi_c) = (c0.min(c1).max(0), c0.max(c1).min(cols - 1));
                let mut r = lo_r;
                while r <= hi_r {
                    let mut col = lo_c;
                    while col <= hi_c {
                        let idx = r * cols + col;
                        if idx >= 0 && idx < n {
                            t.selected[idx as usize] = true;
                        }
                        col += 1;
                    }
                    r += 1;
                }
            }
            ui.global::<AppState>().set_active_pane("right".into());
            ui_bridge::refresh_right_selection(&ui, &c.borrow());
            ui_bridge::update_selection_pane(&ui, &c.borrow(), true);
        }
    });

    // 右面板清空选择
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_clear_selection(move || {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                for s in core.right_pane.selected.iter_mut() {
                    *s = false;
                }
            }
            ui_bridge::refresh_right_selection(&ui, &c.borrow());
            ui_bridge::update_selection_pane(&ui, &c.borrow(), true);
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_go_back(move || {
        if let Some(ui) = w.upgrade() {
            let moved = c.borrow_mut().right_pane.history.go_back().is_some();
            if moved {
                load_right(&ui, &c);
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_go_forward(move || {
        if let Some(ui) = w.upgrade() {
            let moved = c.borrow_mut().right_pane.history.go_forward().is_some();
            if moved {
                load_right(&ui, &c);
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_go_up(move || {
        if let Some(ui) = w.upgrade() {
            let parent = c
                .borrow()
                .right_pane
                .history
                .current()
                .parent()
                .map(|p| p.to_path_buf());
            if let Some(p) = parent {
                navigate_right(&ui, &c, p);
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_refresh(move || {
        if let Some(ui) = w.upgrade() {
            load_right(&ui, &c);
        }
    });
}

/// 打开文件，并把被启动程序的工作目录设为文件所在目录。
/// 这样脚本/程序用相对路径读取同级文件（如 a.py 读取 config.json）才能命中，
/// 否则会继承 FerroxPlorer 自身的工作目录导致“找不到文件”。
#[cfg(windows)]
fn open_with_cwd(path: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // 文件所在目录作为工作目录（lpDirectory）
    let parent = Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    // 转为以 NUL 结尾的 UTF-16
    let to_wide = |s: &OsStr| -> Vec<u16> { s.encode_wide().chain(std::iter::once(0)).collect() };
    let file_w = to_wide(OsStr::new(path));
    let dir_w = to_wide(parent.as_os_str());
    let op_w: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op_w.as_ptr(),
            file_w.as_ptr(),
            std::ptr::null(),
            dir_w.as_ptr(),
            SW_SHOWNORMAL,
        );
    }
}

/// 非 Windows 平台回退到默认打开方式（不强制设置工作目录）。
#[cfg(not(windows))]
fn open_with_cwd(path: &str) {
    let _ = open::that(path);
}

/// 求上一级路径：device:// 走 WPD 父对象逻辑，普通路径用 Path::parent。
fn parent_of(cur: &Path) -> Option<PathBuf> {
    let s = cur.to_string_lossy();
    if s.starts_with("device://") {
        return fs::devices::parent_path(&s).map(PathBuf::from);
    }
    cur.parent().map(|p| p.to_path_buf())
}

/// 双击便携设备文件：后台把文件复制到临时目录，完成后用系统默认程序打开。
/// 复制可能较慢（从手机传输），放后台避免界面卡顿，并通过状态栏反馈进度。
fn open_device_file(ui: &MainWindow, vpath: &str) {
    ui.global::<AppState>()
        .set_status_text("正在从设备复制文件…".into());
    let weak = ui.as_weak();
    let vpath = vpath.to_string();
    std::thread::spawn(move || {
        let result = fs::devices::copy_to_temp(&vpath);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak.upgrade() else { return };
            match result {
                Some(p) => {
                    ui.global::<AppState>()
                        .set_status_text("已打开设备文件".into());
                    let _ = open::that(&p);
                }
                None => {
                    ui.global::<AppState>()
                        .set_status_text("无法打开设备文件".into());
                }
            }
        });
    });
}

// ─── 导航 ───

fn bind_navigation(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    let w = ui.as_weak();
    let c = core.clone();
    state.on_navigate(move |path| {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                navigate_right(&ui, &c, PathBuf::from(path.as_str()));
            } else {
                navigate_to(&ui, &c, PathBuf::from(path.as_str()));
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_commit_path(move |path| {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                navigate_right(&ui, &c, PathBuf::from(path.as_str()));
            } else {
                navigate_to(&ui, &c, PathBuf::from(path.as_str()));
            }
        }
    });

    // Omnibar 路径补全 / > 命令模式：edit 模式输入变化时回填候选
    let w = ui.as_weak();
    let c = core.clone();
    state.on_request_path_completion(move |input| {
        if let Some(ui) = w.upgrade() {
            let st = ui.global::<AppState>();
            let t = input.to_string();
            if let Some(q) = t.strip_prefix('>') {
                // > 命令模式：复用命令面板过滤填充 palette-commands
                st.set_omni_cmd_mode(true);
                st.set_path_completions(slint::ModelRc::new(slint::VecModel::from(
                    Vec::<Crumb>::new(),
                )));
                st.invoke_palette_query(q.trim_start().into());
                st.set_omni_cmd_selected(0);
            } else {
                st.set_omni_cmd_mode(false);
                let base = {
                    let core = c.borrow();
                    if toolbar_routes_right(&ui) {
                        core.right_pane.history.current().clone()
                    } else {
                        core.active_tab().history.current().clone()
                    }
                };
                let comps = compute_path_completions(&t, &base);
                st.set_path_completions(slint::ModelRc::new(slint::VecModel::from(comps)));
            }
        }
    });

    // 添加网络位置（SMB 挂载到空闲盘符）：读对话框输入 -> 挂载 -> 存配置 -> 导航
    let w = ui.as_weak();
    let c = core.clone();
    state.on_add_network_location(move || {
        if let Some(ui) = w.upgrade() {
            let st = ui.global::<AppState>();
            let name = st.get_netloc_name().to_string();
            let server = st.get_netloc_server().to_string();
            let user = st.get_netloc_user().to_string();
            let pass = st.get_netloc_pass().to_string();
            if server.is_empty() {
                st.set_status_text("请输入服务器地址（如 \\\\server\\share）".into());
                return;
            }
            let disp_name = if name.is_empty() {
                server.clone()
            } else {
                name
            };
            match crate::fs::network::mount_smb(&server, &user, &pass) {
                Some(drive) => {
                    {
                        let mut core = c.borrow_mut();
                        core.config
                            .network_locations
                            .push(crate::config::NetworkLocation {
                                name: disp_name.clone(),
                                server,
                                kind: "smb".into(),
                                drive: Some(drive.clone()),
                            });
                        core.config.save();
                    }
                    st.set_netloc_dialog_open(false);
                    st.set_netloc_name("".into());
                    st.set_netloc_server("".into());
                    st.set_netloc_user("".into());
                    st.set_netloc_pass("".into());
                    // 刷新设置「云存储账号」页的已保存列表
                    ui_bridge::push_network_locations(&ui, &c.borrow());
                    navigate_to(&ui, &c, PathBuf::from(drive));
                }
                None => {
                    st.set_status_text("挂载失败，请检查地址和凭据".into());
                }
            }
        }
    });

    // 移除网络位置：卸载盘符 + 删除配置
    let w = ui.as_weak();
    let c = core.clone();
    state.on_remove_network_location(move |name| {
        if let Some(ui) = w.upgrade() {
            let name = name.to_string();
            let mut removed: Option<crate::config::NetworkLocation> = None;
            {
                let mut core = c.borrow_mut();
                if let Some(pos) = core
                    .config
                    .network_locations
                    .iter()
                    .position(|l| l.name == name)
                {
                    removed = Some(core.config.network_locations.remove(pos));
                    core.config.save();
                }
            }
            if let Some(loc) = removed {
                if let Some(d) = loc.drive {
                    crate::fs::network::unmount_smb(&d);
                }
            }
            // 刷新设置「云存储账号」页的已保存列表；若正停留在网络位置视图则同步刷新
            ui_bridge::push_network_locations(&ui, &c.borrow());
            let at_network =
                c.borrow().active_tab().history.current().to_string_lossy() == "network://";
            if at_network {
                load_current(&ui, &c);
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_open_entry(move |idx| {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let target = {
                let core = c.borrow();
                core.pane_entry_at(right, idx as usize)
                    .map(|e| (e.is_dir, e.path.clone()))
            };
            if let Some((is_dir, path)) = target {
                if is_dir {
                    if right {
                        navigate_right(&ui, &c, PathBuf::from(path));
                    } else {
                        navigate_to(&ui, &c, PathBuf::from(path));
                    }
                } else if path.starts_with("device://") {
                    open_device_file(&ui, &path);
                } else {
                    open_with_cwd(&path);
                }
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_go_back(move || {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                let moved = c.borrow_mut().right_pane.history.go_back().is_some();
                if moved {
                    load_right(&ui, &c);
                }
            } else {
                let moved = c.borrow_mut().active_tab_mut().history.go_back().is_some();
                if moved {
                    load_current(&ui, &c);
                }
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_go_forward(move || {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                let moved = c.borrow_mut().right_pane.history.go_forward().is_some();
                if moved {
                    load_right(&ui, &c);
                }
            } else {
                let moved = c
                    .borrow_mut()
                    .active_tab_mut()
                    .history
                    .go_forward()
                    .is_some();
                if moved {
                    load_current(&ui, &c);
                }
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_go_up(move || {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                let cur = c.borrow().right_pane.history.current().clone();
                if let Some(p) = parent_of(&cur) {
                    navigate_right(&ui, &c, p);
                }
            } else {
                let cur = c.borrow().active_tab().history.current().clone();
                if let Some(p) = parent_of(&cur) {
                    navigate_to(&ui, &c, p);
                }
            }
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_refresh(move || {
        if let Some(ui) = w.upgrade() {
            if toolbar_routes_right(&ui) {
                load_right(&ui, &c);
            } else {
                load_current(&ui, &c);
            }
        }
    });

    // 折叠 / 展开侧边栏分区
    let w = ui.as_weak();
    let c = core.clone();
    state.on_toggle_section(move |label| {
        if let Some(ui) = w.upgrade() {
            let path = {
                let mut core = c.borrow_mut();
                let key = label.to_string();
                if !core.collapsed_sections.remove(&key) {
                    core.collapsed_sections.insert(key);
                }
                core.active_tab().history.current().clone()
            };
            let c2 = c.borrow();
            ui.global::<AppState>()
                .set_nav_items(ui_bridge::build_sidebar(
                    &path,
                    &c2.collapsed_sections,
                    &c2.config,
                ));
        }
    });
}

// ─── 选择 ───

fn bind_selection(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    let w = ui.as_weak();
    let c = core.clone();
    state.on_select_entry(move |idx, ctrl| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let tab = core.active_tab_mut();
                let i = idx as usize;
                if i >= tab.selected.len() {
                    return;
                }
                if ctrl {
                    tab.selected[i] = !tab.selected[i];
                } else {
                    for s in tab.selected.iter_mut() {
                        *s = false;
                    }
                    tab.selected[i] = true;
                }
                tab.last_clicked = Some(i);
            }
            // 选中左侧条目时，双面板模式下激活左面板
            let state = ui.global::<AppState>();
            if state.get_dual_pane() {
                state.set_active_pane("left".into());
            }
            ui_bridge::refresh_selection(&ui, &c.borrow());
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_select_range(move |idx| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let tab = core.active_tab_mut();
                let i = idx as usize;
                let anchor = tab.last_clicked.unwrap_or(i);
                let (lo, hi) = if anchor <= i {
                    (anchor, i)
                } else {
                    (i, anchor)
                };
                for (k, s) in tab.selected.iter_mut().enumerate() {
                    *s = k >= lo && k <= hi;
                }
            }
            ui_bridge::refresh_selection(&ui, &c.borrow());
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_select_all(move || {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                for s in core.active_tab_mut().selected.iter_mut() {
                    *s = true;
                }
            }
            ui_bridge::refresh_selection(&ui, &c.borrow());
        }
    });

    // 框选（活动标签/左面板）：把矩形子区 [r0..r1] × [c0..c1]（idx = r*cols+c）置为选中。
    // additive 为 false 时先清空既有选择（普通框选），为 true 时追加（Ctrl+框选）。
    let w = ui.as_weak();
    let c = core.clone();
    state.on_box_select(move |r0, r1, c0, c1, cols, additive| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let tab = core.active_tab_mut();
                let n = tab.selected.len() as i32;
                if !additive {
                    for s in tab.selected.iter_mut() {
                        *s = false;
                    }
                }
                let cols = cols.max(1);
                let (lo_r, hi_r) = (r0.min(r1).max(0), r0.max(r1));
                let (lo_c, hi_c) = (c0.min(c1).max(0), c0.max(c1).min(cols - 1));
                let mut r = lo_r;
                while r <= hi_r {
                    let mut col = lo_c;
                    while col <= hi_c {
                        let idx = r * cols + col;
                        if idx >= 0 && idx < n {
                            tab.selected[idx as usize] = true;
                        }
                        col += 1;
                    }
                    r += 1;
                }
            }
            let state = ui.global::<AppState>();
            if state.get_dual_pane() {
                state.set_active_pane("left".into());
            }
            ui_bridge::refresh_selection(&ui, &c.borrow());
        }
    });

    // 清空选择（点击空白处）：与 select-entry 一致，双面板下点击左面板空白
    // 也切换活动面板，使 sel-* 全局状态跟随「最近交互面板」
    let w = ui.as_weak();
    let c = core.clone();
    state.on_clear_selection(move || {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                for s in core.active_tab_mut().selected.iter_mut() {
                    *s = false;
                }
            }
            let state = ui.global::<AppState>();
            if state.get_dual_pane() {
                state.set_active_pane("left".into());
            }
            ui_bridge::refresh_selection(&ui, &c.borrow());
        }
    });
}

// ─── 文件操作 ───

fn bind_operations(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 复制（双面板下取活动面板的选中项：右侧活动 → right_pane，否则活动标签）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_copy_selected(move || {
        if let Some(ui) = w.upgrade() {
            let has_clips = {
                let mut core = c.borrow_mut();
                core.clipboard = if toolbar_routes_right(&ui) {
                    core.right_pane.selected_paths()
                } else {
                    core.selected_paths()
                };
                core.clip_mode = ClipMode::Copy;
                !core.clipboard.is_empty()
            };
            // 同步「粘贴」按钮可用性
            ui.global::<AppState>().set_can_paste(has_clips);
        }
    });

    // 剪切（同上，按活动面板取选中项）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_cut_selected(move || {
        if let Some(ui) = w.upgrade() {
            let has_clips = {
                let mut core = c.borrow_mut();
                core.clipboard = if toolbar_routes_right(&ui) {
                    core.right_pane.selected_paths()
                } else {
                    core.selected_paths()
                };
                core.clip_mode = ClipMode::Cut;
                !core.clipboard.is_empty()
            };
            ui.global::<AppState>().set_can_paste(has_clips);
        }
    });

    // 粘贴：入队为后台任务（复制 / 移动），由工作线程异步执行并上报真实进度
    let w = ui.as_weak();
    let c = core.clone();
    state.on_paste_here(move || {
        if let Some(ui) = w.upgrade() {
            // 目标目录取活动面板（右侧活动 → right_pane 当前目录，否则活动标签）
            let routes_right = toolbar_routes_right(&ui);
            let (clips, mode, dst) = {
                let core = c.borrow();
                let dst = if routes_right {
                    core.right_pane.history.current().clone()
                } else {
                    core.active_tab().history.current().clone()
                };
                (core.clipboard.clone(), core.clip_mode, dst)
            };
            if mode == ClipMode::None || clips.is_empty() {
                return;
            }
            let kind = if mode == ClipMode::Cut {
                fs::tasks::TaskKind::Move
            } else {
                fs::tasks::TaskKind::Copy
            };
            {
                let mut core = c.borrow_mut();
                // 剪切=移动：乐观记录撤销（源→目标，撤销时移回）。假定无同名冲突重命名。
                if mode == ClipMode::Cut {
                    let pairs: Vec<(PathBuf, PathBuf)> = clips
                        .iter()
                        .filter_map(|src| src.file_name().map(|n| (src.clone(), dst.join(n))))
                        .collect();
                    core.record_undo(app::UndoAction::Move { pairs });
                }
                core.task_queue.push_back(fs::tasks::Job {
                    kind,
                    srcs: clips,
                    dst,
                });
                // 剪切粘贴后清空剪贴板，避免重复移动
                if mode == ClipMode::Cut {
                    core.clipboard.clear();
                    core.clip_mode = ClipMode::None;
                }
            }
            if mode == ClipMode::Cut {
                ui.global::<AppState>().set_can_paste(false);
            }
            start_next_job(&ui, &c);
        }
    });

    // 跨面板拖放：把源面板选中项复制（默认）/ 移动（Ctrl）到另一面板当前目录
    let w = ui.as_weak();
    let c = core.clone();
    state.on_pane_drop(move |source, ctrl| {
        if let Some(ui) = w.upgrade() {
            let src_is_right = source == "right";
            let (srcs, dst) = {
                let core = c.borrow();
                let srcs = if src_is_right {
                    core.right_pane.selected_paths()
                } else {
                    core.selected_paths()
                };
                // 目标为另一面板的当前目录
                let dst = if src_is_right {
                    core.active_tab().history.current().clone()
                } else {
                    core.right_pane.history.current().clone()
                };
                (srcs, dst)
            };
            // 源为空或目标非真实目录（虚拟路径等）时忽略
            if srcs.is_empty() || !dst.is_dir() {
                return;
            }
            let kind = if ctrl {
                fs::tasks::TaskKind::Move
            } else {
                fs::tasks::TaskKind::Copy
            };
            c.borrow_mut()
                .task_queue
                .push_back(fs::tasks::Job { kind, srcs, dst });
            start_next_job(&ui, &c);
        }
    });

    // 任务暂停 / 继续切换
    let w = ui.as_weak();
    let c = core.clone();
    state.on_task_pause(move || {
        if let Some(ui) = w.upgrade() {
            let paused = {
                let core = c.borrow();
                core.task_control.as_ref().map(|ct| ct.toggle_pause())
            };
            if let Some(p) = paused {
                ui.global::<AppState>().set_task_paused(p);
            }
        }
    });

    // 取消当前任务并清空后续排队
    let c = core.clone();
    state.on_task_cancel(move || {
        let mut core = c.borrow_mut();
        if let Some(ct) = &core.task_control {
            ct.cancel();
        }
        core.task_queue.clear();
    });

    // 任务完成（工作线程经事件循环回调）：刷新目录、串联下一项或收起卡片
    let w = ui.as_weak();
    let c = core.clone();
    state.on_task_finished(move |_ok, msg| {
        if let Some(ui) = w.upgrade() {
            c.borrow_mut().task_control = None;
            load_current(&ui, &c);
            // 双面板时右侧面板也可能是任务的源或目标，一并刷新
            if ui.global::<AppState>().get_dual_pane() {
                load_right(&ui, &c);
            }
            let st = ui.global::<AppState>();
            st.set_status_text(msg);
            let has_more = !c.borrow().task_queue.is_empty();
            if has_more {
                start_next_job(&ui, &c);
            } else {
                st.set_task_active(false);
            }
        }
    });

    // 删除
    let w = ui.as_weak();
    let c = core.clone();
    state.on_delete_selected(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            // 「此电脑」视图中选中的是驱动器/设备，禁止删除
            let at_this_pc = c.borrow().pane(right).history.current().to_string_lossy()
                == fs::virtualfs::THIS_PC_PATH;
            if at_this_pc {
                ui.global::<AppState>()
                    .set_status_text("此电脑中的驱动器与设备无法删除".into());
                return;
            }
            let paths = c.borrow().pane_selected_paths(right);
            // 防御：过滤驱动器根路径（如 C:\），避免任何入口误删整盘
            let paths: Vec<PathBuf> = paths
                .into_iter()
                .filter(|p| {
                    let s = p.to_string_lossy();
                    !(s.len() == 3 && s.as_bytes()[1] == b':')
                })
                .collect();
            if paths.is_empty() {
                return;
            }
            let n = paths.len();
            // 移入回收站（可还原），与资源管理器一致
            let msg = match fs::recyclebin::move_to_recycle_bin(&paths) {
                Ok(_) => {
                    c.borrow_mut().record_undo(app::UndoAction::Delete {
                        paths: paths.clone(),
                    });
                    format!("已将 {} 个项目移入回收站", n)
                }
                Err(e) => format!("删除失败：{}", e),
            };
            reload_active_pane(&ui, &c);
            ui.global::<AppState>().set_status_text(msg.into());
        }
    });

    // 重命名提交：回调自带面板语义（rename-entry=左 / r-rename-entry=右），
    // 不按提交瞬间的 active-pane 路由——编辑中途点击另一面板不会改错对象
    let w = ui.as_weak();
    let c = core.clone();
    state.on_rename_entry(move |idx, new_name| {
        if let Some(ui) = w.upgrade() {
            rename_in_pane(&ui, &c, false, idx, new_name.as_str());
        }
    });
    let w = ui.as_weak();
    let c = core.clone();
    state.on_r_rename_entry(move |idx, new_name| {
        if let Some(ui) = w.upgrade() {
            rename_in_pane(&ui, &c, true, idx, new_name.as_str());
        }
    });

    // 新建文件夹
    let w = ui.as_weak();
    let c = core.clone();
    state.on_new_folder(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let dst = c.borrow().pane(right).history.current().clone();
            if let Ok(path) = ops::new_folder(&dst, "新建文件夹") {
                c.borrow_mut().record_undo(app::UndoAction::Create { path });
            }
            reload_active_pane(&ui, &c);
        }
    });

    // 新建文件
    let w = ui.as_weak();
    let c = core.clone();
    state.on_new_file(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let dst = c.borrow().pane(right).history.current().clone();
            if let Ok(path) = ops::new_file(&dst, "新建文本文档.txt") {
                c.borrow_mut().record_undo(app::UndoAction::Create { path });
            }
            reload_active_pane(&ui, &c);
        }
    });

    // 撤销最近一次可逆操作（Ctrl+Z）：弹撤销栈执行逆操作，压入重做栈
    let w = ui.as_weak();
    let c = core.clone();
    state.on_undo(move || {
        if let Some(ui) = w.upgrade() {
            let action = c.borrow_mut().undo_stack.pop();
            let Some(action) = action else {
                ui.global::<AppState>()
                    .set_status_text("没有可撤销的操作".into());
                return;
            };
            let msg = apply_undo(&action);
            c.borrow_mut().redo_stack.push(action);
            load_current(&ui, &c);
            ui.global::<AppState>().set_status_text(msg.into());
        }
    });

    // 重做最近一次被撤销的操作（Ctrl+Shift+Z）：弹重做栈执行正向，压回撤销栈
    let w = ui.as_weak();
    let c = core.clone();
    state.on_redo(move || {
        if let Some(ui) = w.upgrade() {
            let action = c.borrow_mut().redo_stack.pop();
            let Some(action) = action else {
                ui.global::<AppState>()
                    .set_status_text("没有可重做的操作".into());
                return;
            };
            let msg = apply_redo(&action);
            c.borrow_mut().undo_stack.push(action);
            load_current(&ui, &c);
            ui.global::<AppState>().set_status_text(msg.into());
        }
    });

    // F2 请求重命名当前选中项（按活动面板路由到对应的行内编辑）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_request_rename(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let idx = c.borrow().pane(right).first_selected();
            if let Some(i) = idx {
                if right {
                    ui.invoke_set_editing_right(i as i32);
                } else {
                    ui.invoke_set_editing(i as i32);
                }
            }
        }
    });

    // 打开属性（数据源按活动面板：右面板活动时显示右面板选中项）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_open_properties(move |idx| {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            {
                let mut core = c.borrow_mut();
                let tab = core.pane_mut(right);
                let i = idx as usize;
                if i < tab.selected.len() {
                    for s in tab.selected.iter_mut() {
                        *s = false;
                    }
                    tab.selected[i] = true;
                }
            }
            ui_bridge::update_selection_pane(&ui, &c.borrow(), right);
            // 详细信息页：解析当前选中项扩展元数据（打开对话框时一次性填充）
            {
                let core = c.borrow();
                let tab = core.pane(right);
                if let Some(p) =
                    tab.selected
                        .iter()
                        .enumerate()
                        .find(|(_, &s)| s)
                        .and_then(|(fi, _)| {
                            tab.filtered.get(fi).map(|&ei| tab.entries[ei].path.clone())
                        })
                {
                    ui_bridge::fill_details(&ui.global::<AppState>(), Path::new(&p));
                }
            }
            let st = ui.global::<AppState>();
            let empty = vec![
                HashResult {
                    algo: "MD5".into(),
                    value: "".into(),
                },
                HashResult {
                    algo: "SHA-1".into(),
                    value: "".into(),
                },
                HashResult {
                    algo: "SHA-256".into(),
                    value: "".into(),
                },
                HashResult {
                    algo: "SHA-512".into(),
                    value: "".into(),
                },
            ];
            st.set_hashes(slint::ModelRc::new(slint::VecModel::from(empty)));
            ui.set_props_open(true);
        }
    });
}

/// 为"新增"菜单条目推导内置矢量图标类别（与文件列表分类一致）
fn shell_new_icon_class(ext: &str) -> (String, String) {
    if ext.is_empty() {
        return ("folder".into(), "F".into());
    }
    // 借用文件分类逻辑：构造一个仅含扩展名的虚拟路径
    let fake = std::path::PathBuf::from(format!("x{}", ext));
    let (class, label, _kind) = fs::metadata::classify(&fake, false);
    (class, label)
}

// ─── "新增"菜单：枚举系统注册表 ShellNew 项，下拉选择后创建并进入重命名 ───

fn bind_new_menu(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 启动时枚举一次系统"新建"模板（注册表项在会话内基本不变），共享给创建回调使用
    let items = Rc::new(fs::shell_new::enumerate());

    // 推送到 UI 下拉菜单：先填内置矢量图标（保证绝不出现空白/白板）
    let entries: Vec<ShellNewEntry> = items
        .iter()
        .map(|it| {
            let (icon_class, icon_label) = shell_new_icon_class(&it.ext);
            ShellNewEntry {
                name: it.name.clone().into(),
                icon_class: icon_class.into(),
                icon_label: icon_label.into(),
                thumb: slint::Image::default(),
                has_thumb: false,
            }
        })
        .collect();
    state.set_new_menu_items(slint::ModelRc::new(slint::VecModel::from(entries)));

    // 图标来源 = "系统图标"：后台逐个按文件类型提取系统图标，回填到菜单模型，
    // 覆盖内置矢量图。失败的条目保持内置矢量图（不会出现空白）。
    let system_icons = core.borrow().config.settings.icon_source == "system";
    if system_icons {
        let exts: Vec<String> = items.iter().map(|it| it.ext.clone()).collect();
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            for (row, ext) in exts.into_iter().enumerate() {
                let Some((pixels, w, h)) = fs::thumbnail::extract_type_icon(&ext, 32) else {
                    continue;
                };
                let weak2 = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(ui) = weak2.upgrade() else { return };
                    let model = ui.global::<AppState>().get_new_menu_items();
                    if let Some(mut entry) = model.row_data(row) {
                        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                        buf.make_mut_bytes().copy_from_slice(&pixels);
                        entry.thumb = slint::Image::from_rgba8(buf);
                        entry.has_thumb = true;
                        model.set_row_data(row, entry);
                    }
                });
            }
        });
    }

    // 创建选中的"新建"项：在当前目录创建文件/文件夹，刷新后选中并进入行内重命名
    let w = ui.as_weak();
    let c = core.clone();
    let items_for_cb = items.clone();
    state.on_create_new_item(move |index| {
        let Some(ui) = w.upgrade() else { return };
        let Some(item) = items_for_cb.get(index as usize) else {
            return;
        };

        // 按活动面板路由：双面板右侧活动时在右面板当前目录新建
        let right = toolbar_routes_right(&ui);
        let dst = c.borrow().pane(right).history.current().clone();
        // 虚拟位置（回收站 / 标签 / 网络）无法新建实体文件
        if fs::virtualfs::is_virtual(&dst.to_string_lossy()) {
            ui.global::<AppState>()
                .set_status_text("当前位置无法新建项目".into());
            return;
        }

        let created = match fs::shell_new::create_item(&dst, item) {
            Ok(path) => path,
            Err(e) => {
                ui.global::<AppState>()
                    .set_status_text(format!("新建失败：{}", e).into());
                return;
            }
        };
        c.borrow_mut().record_undo(app::UndoAction::Create {
            path: created.clone(),
        });

        reload_active_pane(&ui, &c);

        // 在刷新后的列表中定位新建项（filtered 下标），选中并进入行内重命名
        // （左右面板各有独立的 editing-index / r-editing-index）
        let created_str = created.to_string_lossy().to_string();
        let new_idx = {
            let core = c.borrow();
            let tab = core.pane(right);
            tab.filtered
                .iter()
                .position(|&ei| tab.entries[ei].path == created_str)
        };
        if let Some(i) = new_idx {
            {
                let mut core = c.borrow_mut();
                let tab = core.pane_mut(right);
                for s in tab.selected.iter_mut() {
                    *s = false;
                }
                if i < tab.selected.len() {
                    tab.selected[i] = true;
                }
            }
            if right {
                ui_bridge::refresh_right_selection(&ui, &c.borrow());
                ui.invoke_set_editing_right(i as i32);
            } else {
                ui_bridge::update_selection(&ui, &c.borrow());
                ui.invoke_set_editing(i as i32);
            }
        }
        ui.global::<AppState>()
            .set_status_text(format!("已新建「{}」", item.name).into());
    });
}

// ─── 右键菜单扩展操作：新标签页打开 / 新面板打开 / 压缩 ZIP / 系统原生菜单 ───

fn bind_context_menu_ext(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 在新标签页中打开：文件夹进入该目录；文件进入其父目录
    let w = ui.as_weak();
    let c = core.clone();
    state.on_open_in_new_tab(move |idx| {
        if let Some(ui) = w.upgrade() {
            let target = {
                let core = c.borrow();
                core.entry_at(idx as usize)
                    .map(|e| (e.is_dir, e.path.clone()))
            };
            if let Some((is_dir, path)) = target {
                let dir = if is_dir {
                    PathBuf::from(&path)
                } else {
                    Path::new(&path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from(&path))
                };
                c.borrow_mut().new_tab(dir);
                load_current(&ui, &c);
            }
        }
    });

    // 在新面板中打开：进入该文件夹并切换到双面板视图
    let w = ui.as_weak();
    let c = core.clone();
    state.on_open_in_new_panel(move |idx| {
        if let Some(ui) = w.upgrade() {
            let target = {
                let core = c.borrow();
                core.entry_at(idx as usize)
                    .map(|e| (e.is_dir, e.path.clone()))
            };
            if let Some((is_dir, path)) = target {
                let dir = if is_dir {
                    PathBuf::from(&path)
                } else {
                    Path::new(&path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from(&path))
                };
                // 在右侧独立面板打开该目录，并开启双面板（与布局正交）
                navigate_right(&ui, &c, dir);
                ui.global::<AppState>().set_dual_pane(true);
            }
        }
    });

    // 压缩为 ZIP：压缩选中项（若无选中则压缩目标项）到当前目录，后台任务执行
    let w = ui.as_weak();
    let c = core.clone();
    state.on_compress_selected(move |idx| {
        if let Some(ui) = w.upgrade() {
            enqueue_compress(&ui, &c, idx, "zip");
        }
    });

    // 压缩为指定格式（ActionBar「压缩」下拉：zip / 7z / tar / targz）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_compress_selected_fmt(move |idx, fmt| {
        if let Some(ui) = w.upgrade() {
            enqueue_compress(&ui, &c, idx, fmt.as_str());
        }
    });

    // 解压选中归档到以归档名命名的子文件夹：逐归档入队一个后台任务
    // （进度/速度/ETA、暂停/取消、同名冲突询问均由任务系统提供）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_extract_selected(move || {
        if let Some(ui) = w.upgrade() {
            let (archives, dst) = selected_archives(&ui, &c);
            if archives.is_empty() {
                return;
            }
            {
                let mut core = c.borrow_mut();
                for archive in archives {
                    let stem = archive
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "解压".to_string());
                    // 目标子文件夹以归档名命名，重名自动加序号；入队时即创建目录，
                    // 使同名归档的后续任务能避让到不同序号
                    let target = ops::resolve_conflict(dst.join(&stem));
                    let _ = std::fs::create_dir_all(&target);
                    core.task_queue.push_back(fs::tasks::Job {
                        kind: fs::tasks::TaskKind::Extract,
                        srcs: vec![archive],
                        dst: target,
                    });
                }
            }
            start_next_job(&ui, &c);
        }
    });

    // 解压选中归档到当前文件夹（内容直接落在当前目录，同名走冲突询问）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_extract_selected_here(move || {
        if let Some(ui) = w.upgrade() {
            let (archives, dst) = selected_archives(&ui, &c);
            if archives.is_empty() {
                return;
            }
            {
                let mut core = c.borrow_mut();
                for archive in archives {
                    core.task_queue.push_back(fs::tasks::Job {
                        kind: fs::tasks::TaskKind::Extract,
                        srcs: vec![archive],
                        dst: dst.clone(),
                    });
                }
            }
            start_next_job(&ui, &c);
        }
    });

    // 弹出 Windows 原生 Shell 右键菜单（含第三方注册项）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_show_system_menu(move |idx, mx, my| {
        if let Some(ui) = w.upgrade() {
            // 收集作用对象：优先用当前选中项（多选时菜单作用于全部，与资源管理器一致）；
            // 若右键的目标项不在选中集合中，则回退为仅该目标项。
            let right = toolbar_routes_right(&ui);
            let paths: Vec<String> = {
                let core = c.borrow();
                let tab = core.pane(right);
                let target_selected = tab.selected.get(idx as usize).copied().unwrap_or(false);
                if target_selected {
                    core.pane_selected_paths(right)
                        .iter()
                        .map(|p| p.to_string_lossy().to_string())
                        .collect()
                } else {
                    core.pane_entry_at(right, idx as usize)
                        .map(|e| vec![e.path.clone()])
                        .unwrap_or_default()
                }
            };
            if paths.is_empty() {
                return;
            }
            // 弹出原生菜单（阻塞至用户选择/关闭）；若执行了命令（删除/重命名/粘贴等），
            // 刷新活动面板使视图与磁盘状态保持一致。
            let invoked = show_system_context_menu(&ui, &paths, mx, my);
            if invoked {
                reload_active_pane(&ui, &c);
            }
        }
    });

    // 固定到快速访问：调用系统 pintohome 动词写入真实快速访问，并刷新侧边栏
    let w = ui.as_weak();
    let c = core.clone();
    state.on_pin_to_quick_access(move |idx| {
        if let Some(ui) = w.upgrade() {
            let target = {
                let core = c.borrow();
                core.entry_at(idx as usize)
                    .map(|e| (e.is_dir, e.path.clone()))
            };
            let Some((is_dir, path)) = target else { return };
            if !is_dir {
                return; // 仅文件夹可固定
            }
            let ok = fs::quickaccess::pin(&path);
            let c2 = c.borrow();
            // 重建侧边栏，使新固定项立即出现在「快速访问」
            ui.global::<AppState>()
                .set_nav_items(ui_bridge::build_sidebar(
                    c2.active_tab().history.current(),
                    &c2.collapsed_sections,
                    &c2.config,
                ));
            ui.global::<AppState>().set_status_text(
                if ok {
                    "已固定到快速访问"
                } else {
                    "固定到快速访问失败"
                }
                .into(),
            );
        }
    });
}

/// 把窗口内逻辑坐标 (mx,my) 换算为屏幕物理坐标，并弹出系统原生右键菜单。
/// 返回是否执行了某条命令（供调用方决定是否刷新视图）。
#[cfg(windows)]
fn show_system_context_menu(ui: &MainWindow, paths: &[String], mx: f32, my: f32) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let paths = paths.to_vec();
    let mut invoked = false;
    ui.window().with_winit_window(|winit_window| {
        // 窗口左上角屏幕物理坐标
        let origin = match winit_window.inner_position() {
            Ok(p) => p,
            Err(_) => return,
        };
        // 逻辑像素 → 物理像素
        let scale = winit_window.scale_factor() as f32;
        let screen_x = origin.x + (mx * scale).round() as i32;
        let screen_y = origin.y + (my * scale).round() as i32;

        // 取 HWND（isize 形式传入 shell_menu）
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd_isize = isize::from(h.hwnd);
            invoked = fs::shell_menu::show(&paths, hwnd_isize, screen_x, screen_y);
        }
    });
    invoked
}

#[cfg(not(windows))]
fn show_system_context_menu(_ui: &MainWindow, _paths: &[String], _mx: f32, _my: f32) -> bool {
    false
}

// ─── 用户设置：启动推送 + setter 接线 ───

// 配置内部规范值 ↔ UI 中文显示值 的互转
fn lang_disp(c: &str) -> &'static str {
    match c {
        "en" => "English",
        _ => "简体中文",
    }
}
fn lang_canon(d: &str) -> &'static str {
    match d {
        "English" => "en",
        _ => "zh-CN",
    }
}
fn startup_disp(c: &str) -> &'static str {
    match c {
        "quick" => "快速访问",
        "this-pc" => "此电脑",
        _ => "上次的标签页",
    }
}
fn startup_canon(d: &str) -> &'static str {
    match d {
        "快速访问" => "quick",
        "此电脑" => "this-pc",
        _ => "last",
    }
}
fn icon_disp(c: &str) -> &'static str {
    match c {
        "builtin" => "内置图标",
        _ => "系统图标",
    }
}
fn icon_canon(d: &str) -> &'static str {
    match d {
        "内置图标" => "builtin",
        _ => "system",
    }
}
fn view_disp(c: &str) -> &'static str {
    match c {
        "grid" => "网格",
        "dual" => "双面板",
        _ => "详细信息",
    }
}
fn view_canon(d: &str) -> &'static str {
    match d {
        "网格" => "grid",
        "双面板" => "dual",
        _ => "list",
    }
}
fn sort_disp(c: &str) -> &'static str {
    match c {
        "size" => "大小",
        "modified" => "修改日期",
        "kind" => "类型",
        _ => "名称",
    }
}
fn sort_canon(d: &str) -> &'static str {
    match d {
        "大小" => "size",
        "修改日期" => "modified",
        "类型" => "kind",
        _ => "name",
    }
}
fn newtab_disp(c: &str) -> &'static str {
    match c {
        "this-pc" => "此电脑",
        "last" => "上次目录",
        _ => "快速访问",
    }
}
fn newtab_canon(d: &str) -> &'static str {
    match d {
        "此电脑" => "this-pc",
        "上次目录" => "last",
        _ => "quick",
    }
}
fn split_disp(c: &str) -> &'static str {
    match c {
        "40" => "40% / 60%",
        "60" => "60% / 40%",
        _ => "50% / 50%",
    }
}
fn split_canon(d: &str) -> &'static str {
    match d {
        "40% / 60%" => "40",
        "60% / 40%" => "60",
        _ => "50",
    }
}
fn index_disp(c: &str) -> &'static str {
    match c {
        "all" => "全部磁盘",
        "custom" => "自定义",
        _ => "用户目录",
    }
}
fn index_canon(d: &str) -> &'static str {
    match d {
        "全部磁盘" => "all",
        "自定义" => "custom",
        _ => "user",
    }
}

/// 解析 hex 颜色字符串（如 "#0078d4" 或 "#ff6600ff"）为 Slint Color。
/// 支持 #RGB / #RRGGBB / #RRGGBBAA 三种格式，失败返回 None。
fn parse_hex_color(hex: &str) -> Option<slint::Color> {
    let h = hex
        .strip_prefix('#')
        .or_else(|| hex.strip_prefix("0x"))
        .unwrap_or(hex);
    let (r, g, b, a) = match h.len() {
        3 => (
            u8::from_str_radix(&h[0..1].repeat(2), 16).ok()?,
            u8::from_str_radix(&h[1..2].repeat(2), 16).ok()?,
            u8::from_str_radix(&h[2..3].repeat(2), 16).ok()?,
            255u8,
        ),
        6 => (
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
            255u8,
        ),
        8 => (
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
            u8::from_str_radix(&h[6..8], 16).ok()?,
        ),
        _ => return None,
    };
    Some(slint::Color::from_argb_u8(a, r, g, b))
}

/// Slint Color → hex 字符串 "#RRGGBB"
fn color_to_hex(c: slint::Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

/// 启动时把持久化设置推送到 Theme（主题/半透明）与 AppState（其余项）
fn push_settings(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let c = core.borrow();
    let s = &c.config.settings;

    let theme = ui.global::<Theme>();
    theme.set_theme_mode(s.theme_mode.clone().into());
    theme.set_accent_key(s.accent.clone().into());
    // 自定义主题色：解析 hex 字符串回 Slint Color
    theme.set_accent_custom(
        parse_hex_color(&s.accent_custom).unwrap_or(slint::Color::from_rgb_u8(0x00, 0x78, 0xd4)),
    );
    theme.set_translucent(s.translucent);
    theme.set_opacity_level(s.opacity);
    theme.set_blur_level(s.blur);
    theme.set_compact(s.compact_mode);

    let st = ui.global::<AppState>();
    st.set_set_launch_startup(s.launch_on_startup);
    st.set_set_single_click(s.single_click_open);
    st.set_set_language(lang_disp(&s.language).into());
    st.set_set_startup_open(startup_disp(&s.startup_open).into());
    st.set_set_default_fm(s.default_file_manager);
    st.set_set_icon_source(icon_disp(&s.icon_source).into());
    st.set_set_show_hidden(s.show_hidden);
    st.set_set_show_ext(s.show_extensions);
    st.set_set_show_protected(s.show_protected);
    st.set_set_calc_size(s.calc_folder_size);
    st.set_set_folders_first(s.folders_first);
    st.set_set_default_view(view_disp(&s.default_view).into());
    st.set_set_default_sort(sort_disp(&s.default_sort).into());
    st.set_set_restore_tabs(s.restore_tabs);
    st.set_set_exit_last_tab(s.exit_on_last_tab);
    st.set_set_new_tab_loc(newtab_disp(&s.new_tab_location).into());
    st.set_set_show_details_default(s.show_details_default);
    st.set_set_dual_default(s.dual_pane_default);
    st.set_set_split_ratio(split_disp(&s.split_ratio).into());
    st.set_set_live_filter(s.live_filter);
    st.set_set_search_subfolders(s.search_subfolders);
    st.set_set_case_sensitive(s.case_sensitive);
    st.set_set_background_index(s.background_index);
    st.set_set_index_location(index_disp(&s.index_location).into());
    st.set_set_context_menu_system(s.context_menu_system);

    // 默认布局与详情面板显隐：仅启动时应用一次。
    // 布局（grid/list）与双面板正交：旧配置里 default_view 可能为 "dual"，
    // 此时回退为列表布局，双面板开关交由 dual_pane_default 决定。
    let layout = if s.default_view == "grid" {
        "grid"
    } else {
        "list"
    };
    st.set_view_mode(layout.into());
    st.set_dual_pane(s.dual_pane_default || s.default_view == "dual");
    st.set_show_details(s.show_details_default);
    // 图标缩放比例（Ctrl+滚轮），启动时回显
    st.set_icon_scale(s.icon_scale.clamp(0.7, 2.0));
}

/// 绑定设置项 setter：更新配置 → 持久化 → 回显（Theme/AppState）→ 必要时重载目录
fn bind_settings(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // —— 布尔设置 ——
    let w = ui.as_weak();
    let c = core.clone();
    state.on_set_bool(move |key, val| {
        if let Some(ui) = w.upgrade() {
            let mut reload = false;
            {
                let mut core = c.borrow_mut();
                let s = &mut core.config.settings;
                match key.as_str() {
                    "launch-startup" => s.launch_on_startup = val,
                    "single-click" => s.single_click_open = val,
                    "default-fm" => s.default_file_manager = val,
                    "show-hidden" => {
                        s.show_hidden = val;
                        reload = true;
                    }
                    "show-ext" => s.show_extensions = val,
                    "show-protected" => s.show_protected = val,
                    "calc-size" => s.calc_folder_size = val,
                    "folders-first" => {
                        s.folders_first = val;
                        reload = true;
                    }
                    "restore-tabs" => s.restore_tabs = val,
                    "exit-last-tab" => s.exit_on_last_tab = val,
                    "show-details-default" => s.show_details_default = val,
                    "dual-default" => s.dual_pane_default = val,
                    "live-filter" => s.live_filter = val,
                    "search-subfolders" => s.search_subfolders = val,
                    "case-sensitive" => s.case_sensitive = val,
                    "background-index" => s.background_index = val,
                    "context-menu-system" => s.context_menu_system = val,
                    "translucent" => s.translucent = val,
                    "compact" => s.compact_mode = val,
                    _ => {}
                }
                core.config.save();
            }
            // 回显到 UI（Theme 或 AppState 属性），保证视觉与状态一致
            let theme = ui.global::<Theme>();
            let st = ui.global::<AppState>();
            match key.as_str() {
                "translucent" => {
                    theme.set_translucent(val);
                    // 同步开关窗口透明（边框延伸/圆角/标题栏处理）与真实亚克力磨砂（浓度随 blur-level 连续变化）
                    #[cfg(windows)]
                    apply_window_material(&ui);
                }
                "compact" => theme.set_compact(val),
                "launch-startup" => st.set_set_launch_startup(val),
                "single-click" => st.set_set_single_click(val),
                "default-fm" => {
                    // 应用/撤销注册表接管；失败时回滚开关并提示
                    match fs::default_app::set_default(val) {
                        Ok(_) => {
                            st.set_set_default_fm(val);
                            st.set_status_text(
                                if val {
                                    "已设为默认文件管理器 (双击文件夹与 Win+E 将打开本应用)"
                                } else {
                                    "已恢复系统资源管理器为默认"
                                }
                                .into(),
                            );
                        }
                        Err(e) => {
                            st.set_set_default_fm(!val);
                            c.borrow_mut().config.settings.default_file_manager = !val;
                            c.borrow().config.save();
                            st.set_status_text(format!("注册表操作失败: {}", e).into());
                        }
                    }
                }
                "show-hidden" => st.set_set_show_hidden(val),
                "show-ext" => st.set_set_show_ext(val),
                "show-protected" => st.set_set_show_protected(val),
                "calc-size" => st.set_set_calc_size(val),
                "folders-first" => st.set_set_folders_first(val),
                "restore-tabs" => st.set_set_restore_tabs(val),
                "exit-last-tab" => st.set_set_exit_last_tab(val),
                "show-details-default" => st.set_set_show_details_default(val),
                "dual-default" => st.set_set_dual_default(val),
                "live-filter" => st.set_set_live_filter(val),
                "search-subfolders" => st.set_set_search_subfolders(val),
                "case-sensitive" => st.set_set_case_sensitive(val),
                "background-index" => st.set_set_background_index(val),
                "context-menu-system" => st.set_set_context_menu_system(val),
                _ => {}
            }
            if reload {
                load_current(&ui, &c);
            }
        }
    });

    // —— 字符串设置（下拉选项 + 主题模式/主题色）——
    let w = ui.as_weak();
    let c = core.clone();
    state.on_set_string(move |key, val| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let s = &mut core.config.settings;
                match key.as_str() {
                    "theme-mode" => s.theme_mode = val.to_string(),
                    "accent" => s.accent = val.to_string(),
                    "language" => s.language = lang_canon(val.as_str()).into(),
                    "startup-open" => s.startup_open = startup_canon(val.as_str()).into(),
                    "icon-source" => s.icon_source = icon_canon(val.as_str()).into(),
                    "default-view" => s.default_view = view_canon(val.as_str()).into(),
                    "default-sort" => s.default_sort = sort_canon(val.as_str()).into(),
                    "new-tab-loc" => s.new_tab_location = newtab_canon(val.as_str()).into(),
                    "split-ratio" => s.split_ratio = split_canon(val.as_str()).into(),
                    "index-location" => s.index_location = index_canon(val.as_str()).into(),
                    _ => {}
                }
                core.config.save();
            }
            let theme = ui.global::<Theme>();
            let st = ui.global::<AppState>();
            match key.as_str() {
                "theme-mode" => theme.set_theme_mode(val),
                "accent" => theme.set_accent_key(val),
                "language" => st.set_set_language(val),
                "startup-open" => st.set_set_startup_open(val),
                "icon-source" => st.set_set_icon_source(val),
                "default-view" => st.set_set_default_view(val),
                "default-sort" => st.set_set_default_sort(val),
                "new-tab-loc" => st.set_set_new_tab_loc(val),
                "split-ratio" => st.set_set_split_ratio(val),
                "index-location" => st.set_set_index_location(val),
                _ => {}
            }
            // 图标来源切换：重载当前目录以按新策略重新拉取系统图标/缩略图，
            // 并重建侧边栏导航模型——否则侧边栏图标（快速访问/驱动器等）要等下次导航才会更新
            if key.as_str() == "icon-source" {
                load_current(&ui, &c);
                let c2 = c.borrow();
                let path = c2.active_tab().history.current().clone();
                st.set_nav_items(ui_bridge::build_sidebar(
                    &path,
                    &c2.collapsed_sections,
                    &c2.config,
                ));
            }
            // 分隔比例预设：立即应用到双面板左侧占比并持久化
            if key.as_str() == "split-ratio" {
                let ratio = match c.borrow().config.settings.split_ratio.as_str() {
                    "40" => 0.4,
                    "60" => 0.6,
                    _ => 0.5,
                };
                st.set_dual_ratio(ratio);
                let mut core = c.borrow_mut();
                core.config.layout.dual_ratio = ratio;
                core.config.save();
            }
        }
    });

    // —— 数值设置（半透明不透明度 / 磨砂强度）——
    let w = ui.as_weak();
    let c = core.clone();
    state.on_set_number(move |key, val| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                match key.as_str() {
                    "opacity" => core.config.settings.opacity = val,
                    "blur" => core.config.settings.blur = val,
                    // 图标缩放（Ctrl+滚轮）：夹紧到 0.7..2.0 并持久化
                    "icon-scale" => core.config.settings.icon_scale = val.clamp(0.7, 2.0),
                    _ => {}
                }
                core.config.save();
            }
            let theme = ui.global::<Theme>();
            match key.as_str() {
                "opacity" => theme.set_opacity_level(val),
                "blur" => {
                    theme.set_blur_level(val);
                    // 模糊度连续映射到亚克力磨砂浓度（仅 Windows 生效），不再是二元开关
                    #[cfg(windows)]
                    apply_window_material(&ui);
                }
                _ => {}
            }
        }
    });

    // -- 取色板：点击自定义主题色色块时，打开 Windows 原生 ChooseColor 对话框 --
    let w = ui.as_weak();
    let c = core.clone();
    state.on_request_color_pick(move || {
        let Some(ui) = w.upgrade() else { return };
        let theme = ui.global::<Theme>();
        let current = theme.get_accent_custom();

        // 取主窗口 HWND 作为对话框父窗口
        let mut picked: Option<slint::Color> = None;
        ui.window().with_winit_window(|ww| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let Ok(handle) = ww.window_handle() else {
                return;
            };
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let hwnd = isize::from(h.hwnd);
                picked = open_color_picker(current, hwnd);
            }
        });

        if let Some(color) = picked {
            // 更新 Theme
            theme.set_accent_custom(color);
            theme.set_accent_key("custom".into());
            // 持久化
            {
                let mut core = c.borrow_mut();
                core.config.settings.accent = "custom".into();
                core.config.settings.accent_custom = color_to_hex(color);
                core.config.save();
            }
        }
    });
}

// ─── 视图与搜索 ───

fn bind_view_and_search(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    let w = ui.as_weak();
    state.on_set_view(move |mode| {
        if let Some(ui) = w.upgrade() {
            ui.global::<AppState>().set_view_mode(mode);
        }
    });

    // 双面板开关：与布局正交，仅翻转 dual-pane 布尔；开启时刷新右侧面板内容
    let w = ui.as_weak();
    let c = core.clone();
    state.on_toggle_dual(move || {
        if let Some(ui) = w.upgrade() {
            let st = ui.global::<AppState>();
            let on = !st.get_dual_pane();
            st.set_dual_pane(on);
            if on {
                load_right(&ui, &c);
            } else {
                // 关闭双面板时退出行内重命名：RightPane 卸载后 Escape/Enter
                // 无法触达，残留的 r-editing-index 会永久禁用 InputOverlay
                ui.invoke_clear_editing();
            }
        }
    });

    // 交换左右面板：活动标签会话与右侧独立面板整体互换（含导航历史/排序/选中）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_swap_panes(move || {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                // 设置标签页不参与交换（右面板无法承载设置界面）
                if core.active_tab().kind != app::TabKind::Files {
                    return;
                }
                let active = core.active;
                // 显式重借用出 &mut AppCore：RefMut 每次字段访问都经 DerefMut，
                // 直接对两个字段取 &mut 会被判成对整个结构的双重可变借用
                let core = &mut *core;
                std::mem::swap(&mut core.tabs[active], &mut core.right_pane);
            }
            load_current(&ui, &c);
            load_right(&ui, &c);
        }
    });

    // 双面板比例拖拽结束：持久化左侧占比
    let w = ui.as_weak();
    let c = core.clone();
    state.on_save_dual_ratio(move |ratio| {
        if let Some(_ui) = w.upgrade() {
            let mut core = c.borrow_mut();
            core.config.layout.dual_ratio = ratio.clamp(0.15, 0.85);
            core.config.save();
        }
    });

    let w = ui.as_weak();
    state.on_set_omni_mode(move |mode| {
        if let Some(ui) = w.upgrade() {
            ui.global::<AppState>().set_omni_mode(mode);
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_do_search(move |text| {
        if let Some(ui) = w.upgrade() {
            // 清空搜索：重新读取当前目录（深层搜索可能已把 entries 换成索引结果）
            if text.is_empty() {
                {
                    let mut core = c.borrow_mut();
                    core.active_tab_mut().search.clear();
                }
                ui.global::<AppState>().set_search_text(text);
                load_current(&ui, &c);
                return;
            }
            // 深层搜索：开启「搜索子文件夹」且当前为真实目录时，用文件名索引
            // 在当前目录范围内查找；索引未建立时回退到当前目录过滤
            let deep_dir = {
                let core = c.borrow();
                let tab = core.active_tab();
                let path = tab.history.current().clone();
                let is_virtual = fs::virtualfs::is_virtual(&path.to_string_lossy());
                if core.config.settings.search_subfolders && !is_virtual {
                    Some((path, core.config.settings.case_sensitive))
                } else {
                    None
                }
            };
            if let Some((dir, case_sensitive)) = deep_dir {
                if let Some(results) = fs::index::search(&dir, text.as_str(), case_sensitive, 1000)
                {
                    let n = results.len();
                    {
                        let mut core = c.borrow_mut();
                        let tab = core.active_tab_mut();
                        tab.entries = results;
                        tab.search = text.to_string();
                        tab.rebuild();
                    }
                    let st = ui.global::<AppState>();
                    st.set_search_text(text);
                    st.set_status_text(format!("深层搜索：含子文件夹共 {} 个匹配项", n).into());
                    ui_bridge::push_entries(&ui, &c.borrow());
                    return;
                }
                ui.global::<AppState>().set_status_text(
                    "尚未建立索引，已在当前目录过滤;可在 设置 > 搜索与索引 中重建索引".into(),
                );
            }
            {
                let mut core = c.borrow_mut();
                let tab = core.active_tab_mut();
                tab.search = text.to_string();
                tab.rebuild();
            }
            ui.global::<AppState>().set_search_text(text);
            ui_bridge::push_entries(&ui, &c.borrow());
        }
    });

    let w = ui.as_weak();
    state.on_toggle_details(move || {
        if let Some(ui) = w.upgrade() {
            let st = ui.global::<AppState>();
            st.set_show_details(!st.get_show_details());
        }
    });

    let w = ui.as_weak();
    let c = core.clone();
    state.on_sort_by(move |key| {
        if let Some(ui) = w.upgrade() {
            {
                let mut core = c.borrow_mut();
                let tab = core.active_tab_mut();
                if tab.sort_key == key.as_str() {
                    tab.sort_asc = !tab.sort_asc;
                } else {
                    tab.sort_key = key.to_string();
                    tab.sort_asc = true;
                }
                tab.rebuild();
            }
            ui_bridge::push_entries(&ui, &c.borrow());
        }
    });
}

// ─── 哈希 ───

fn bind_hash(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();
    let c = core.clone();
    let w = ui.as_weak();
    state.on_compute_hash(move |algo| {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let core = c.borrow();
            let tab = core.pane(right);
            if let Some(fi) = tab.first_selected() {
                if let Some(e) = tab.entry_at(fi) {
                    if !e.is_dir {
                        return ui_bridge::hash_to_shared(Path::new(&e.path), algo.as_str());
                    }
                }
            }
        }
        "请先选择一个文件".into()
    });

    // 哈希校验：按期望值长度自动识别算法并与选中文件比对，结果回填 AppState
    let c = core.clone();
    let w = ui.as_weak();
    state.on_verify_hash(move |expected| {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let st = ui.global::<AppState>();
            let (text, status) = {
                let core = c.borrow();
                let tab = core.pane(right);
                tab.first_selected()
                    .and_then(|fi| tab.entry_at(fi))
                    .filter(|e| !e.is_dir)
                    .map(|e| ui_bridge::verify_result(Path::new(&e.path), expected.as_str()))
                    .unwrap_or_else(|| ("请先选择一个文件".into(), 3))
            };
            st.set_verify_result(text);
            st.set_verify_status(status);
        }
    });

    // 复制文本到剪贴板（复制哈希值）
    state.on_copy_text(move |text| {
        fs::clipboard::set_text(text.as_str());
    });

    // 「打开方式 - 更改」：弹出系统「打开方式」对话框
    let c = core.clone();
    let w = ui.as_weak();
    state.on_open_with_dialog(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            let path = {
                let core = c.borrow();
                let tab = core.pane(right);
                tab.first_selected()
                    .and_then(|fi| tab.entry_at(fi))
                    .filter(|e| !e.is_dir)
                    .map(|e| e.path.clone())
            };
            if let Some(path) = path {
                show_open_with_dialog(&ui, &path);
            }
        }
    });

    // 空格键 Quick Look：填充预览内容并打开浮层（按活动面板取选中项）
    let c = core.clone();
    let w = ui.as_weak();
    state.on_open_quicklook(move || {
        if let Some(ui) = w.upgrade() {
            let right = toolbar_routes_right(&ui);
            if ui_bridge::fill_quicklook(&ui, &c.borrow(), right) {
                ui.global::<AppState>().set_quicklook_open(true);
            }
        }
    });

    // 关闭 Quick Look
    let w = ui.as_weak();
    state.on_close_quicklook(move || {
        if let Some(ui) = w.upgrade() {
            ui.global::<AppState>().set_quicklook_open(false);
        }
    });
}

/// 弹出系统「打开方式」对话框（取宿主 HWND 后调用 SHOpenWithDialog）。
#[cfg(windows)]
fn show_open_with_dialog(ui: &MainWindow, path: &str) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let path = path.to_string();
    ui.window().with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd_isize = isize::from(h.hwnd);
            fs::openwith::open_with_dialog(&path, hwnd_isize);
        }
    });
}

#[cfg(not(windows))]
fn show_open_with_dialog(_ui: &MainWindow, _path: &str) {}

// ─── 标签页 ───

fn bind_tabs(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let state = ui.global::<AppState>();

    // 新建标签页：起始位置遵循设置「新标签页默认位置」
    let w = ui.as_weak();
    let c = core.clone();
    state.on_new_tab(move || {
        if let Some(ui) = w.upgrade() {
            let start = {
                let core = c.borrow();
                match core.config.settings.new_tab_location.as_str() {
                    "this-pc" => PathBuf::from(fs::virtualfs::THIS_PC_PATH),
                    // 上次目录 = 当前活动标签所在目录
                    "last" => core.active_tab().history.current().clone(),
                    // quick（快速访问）与未知值回退到用户主目录
                    _ => home_start_path(),
                }
            };
            c.borrow_mut().new_tab(start);
            load_current(&ui, &c);
        }
    });

    // 关闭标签页（按下标关闭）
    let w = ui.as_weak();
    let c = core.clone();
    state.on_close_tab_at(move |idx| {
        if let Some(ui) = w.upgrade() {
            if c.borrow_mut().close_tab(idx as usize).is_some() {
                load_current(&ui, &c);
            }
        }
    });

    // 切换标签页
    let w = ui.as_weak();
    let c = core.clone();
    state.on_switch_tab(move |idx| {
        if let Some(ui) = w.upgrade() {
            c.borrow_mut().switch_tab(idx as usize);
            load_current(&ui, &c);
        }
    });

    // 拖动重排标签页
    let w = ui.as_weak();
    let c = core.clone();
    state.on_move_tab(move |from, to| {
        if let Some(ui) = w.upgrade() {
            c.borrow_mut().move_tab(from as usize, to as usize);
            load_current(&ui, &c);
        }
    });

    // 打开（或切换到）设置标签页
    let w = ui.as_weak();
    let c = core.clone();
    state.on_open_settings_tab(move || {
        if let Some(ui) = w.upgrade() {
            c.borrow_mut().open_settings_tab();
            load_current(&ui, &c);
        }
    });
}

// ─── 窗口控制（无边框自绘标题栏）───

/// 把当前窗口位置/大小/最大化状态写回配置（应用关闭或安装更新退出前调用）。
/// 最大化时仅记录标志，不覆盖已保存的常规尺寸——还原后仍回到之前的大小。
fn save_window_geometry(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    let maximized = ui.window().is_maximized();
    let mut c = core.borrow_mut();
    let lay = &mut c.config.layout;
    lay.win_maximized = maximized;
    if !maximized {
        let pos = ui.window().position();
        let size = ui.window().size();
        // 过滤异常值（最小化中关闭等场景可能拿到 0 尺寸）
        if size.width > 200 && size.height > 200 {
            lay.win_x = pos.x;
            lay.win_y = pos.y;
            lay.win_w = size.width as i32;
            lay.win_h = size.height as i32;
        }
    }
    c.config.save();
}

#[cfg(windows)]
fn apply_window_material(ui: &MainWindow) {
    let theme = ui.global::<Theme>();
    let translucent = theme.get_translucent();
    apply_acrylic_backdrop(ui, translucent);
    apply_acrylic_blur_behind(ui, translucent, theme.get_blur_level());
}

#[cfg(windows)]
fn apply_current_window_effects(ui: &MainWindow) {
    apply_window_round_corners(ui);
    apply_window_material(ui);
}

#[cfg(windows)]
fn schedule_window_effects(ui: &MainWindow, delays_ms: &[u64]) {
    for &delay in delays_ms {
        let w = ui.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
            if let Some(ui) = w.upgrade() {
                apply_current_window_effects(&ui);
            }
        });
    }
}

fn bind_window_chrome(ui: &MainWindow, core: &Rc<RefCell<AppCore>>) {
    // 最小化
    let w = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = w.upgrade() {
            ui.window().set_minimized(true);
        }
    });

    // 最大化 / 还原切换
    let w = ui.as_weak();
    ui.on_toggle_maximize(move || {
        if let Some(ui) = w.upgrade() {
            let next = !ui.window().is_maximized();
            ui.window().set_maximized(next);
            ui.set_window_maximized(next);
            // 最大化/还原会触发 FRAMECHANGED，DWM 可能重置非客户区与亚克力策略。
            #[cfg(windows)]
            schedule_window_effects(&ui, &[80]);
        }
    });

    // 关闭窗口：先保存窗口几何，再退出事件循环。
    let w = ui.as_weak();
    let c = core.clone();
    ui.on_close_window(move || {
        if let Some(ui) = w.upgrade() {
            save_window_geometry(&ui, &c);
            let _ = slint::quit_event_loop();
        }
    });

    // 系统关闭请求（Alt+F4、任务栏缩略图关闭等）同样保存，避免绕过自绘按钮。
    let w = ui.as_weak();
    let c = core.clone();
    ui.window().on_close_requested(move || {
        if let Some(ui) = w.upgrade() {
            save_window_geometry(&ui, &c);
        }
        slint::CloseRequestResponse::HideWindow
    });

    // 拖动窗口：在标题栏空白处按下时调用 winit drag_window
    let w = ui.as_weak();
    ui.on_start_window_drag(move || {
        if let Some(ui) = w.upgrade() {
            ui.window().with_winit_window(|winit_window| {
                let _ = winit_window.drag_window();
            });
        }
    });

    // Windows 11：窗口创建、首次显示和 DWM 首轮合成可能分阶段完成；有限重试并始终
    // 读取当前 Theme，确保冷启动恢复的模糊度不会被后续窗口样式更新覆盖。
    #[cfg(windows)]
    {
        // 图标是稳定窗口身份，仅设置一次；DWM 合成效果才需要有限重试。
        set_window_icon(ui);
        schedule_window_effects(ui, &[60, 250, 800]);
    }
}

/// 开启/关闭窗口透明所需的「玻璃基座」：把 DWM 边框延伸到整个客户区，
/// 使透明像素后方允许合成层显示内容；真正的磨砂浓度由 `apply_acrylic_blur_behind`
/// 通过 SetWindowCompositionAttribute 连续控制（DWMWA_SYSTEMBACKDROP_TYPE 恒设为
/// DWMSBT_NONE，避免与之重复合成打架）。
#[cfg(windows)]
fn apply_acrylic_backdrop(ui: &MainWindow, translucent: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
    use windows_sys::Win32::UI::Controls::MARGINS;

    // windows-sys 0.59 未导出该枚举常量，按官方数值硬编码
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_NONE: i32 = 1;

    ui.window().with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd = isize::from(h.hwnd) as HWND;
            unsafe {
                // 关键：无边框窗口默认边框为 0，透明像素后方不会被合成。用 -1 把边框
                // 延伸到整个客户区（"sheet of glass"），DWM 才会在客户区透明像素后方
                // 合成 SetWindowCompositionAttribute 绘制的亚克力磨砂；关闭时归零，
                // 恢复纯色窗口。
                let inset: i32 = if translucent { -1 } else { 0 };
                let margins = MARGINS {
                    cxLeftWidth: inset,
                    cxRightWidth: inset,
                    cyTopHeight: inset,
                    cyBottomHeight: inset,
                };
                DwmExtendFrameIntoClientArea(hwnd, &margins);

                // 磨砂浓度改由 apply_acrylic_blur_behind 提供连续控制，这里固定关闭
                // DWM 自身的系统背景，避免两套亚克力合成叠加出异常观感。
                let backdrop: i32 = DWMSBT_NONE;
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop as *const i32 as *const core::ffi::c_void,
                    std::mem::size_of::<i32>() as u32,
                );
            }
        }
    });

    // 把边框延伸到整个客户区会重新启用 DWM 非客户区绘制，导致系统又画出一套
    // Windows 11 原生标题栏按钮。延伸边框后立即剥离 WS_SYSMENU 消除它们。
    if translucent {
        strip_native_caption_buttons(ui);
    }
}

/// 通过非公开 API `SetWindowCompositionAttribute`（user32.dll 导出，TranslucentTB、
/// 旧版 Windows Terminal 等均在用）驱动亚克力磨砂。
///
/// **重要限制**：这个 API 能调的只有 tint 颜色与 tint 的混合浓度（`GradientColor`
/// 的 alpha），实际的高斯模糊半径由 DWM 内部固定、系统层面不提供任何调节手段——
/// 这是 Windows 平台本身的限制，并非本程序未实现。此前版本把 alpha 从 24 线性拉
/// 到 220，且 tint 的 RGB 直接取了偏白的 `Theme.bg`（浅色下 `#edf3f9`），后果就是
/// 用户反馈的两个问题：一是 0 → 略大于0 时 accent_state 从
/// TRANSPARENTGRADIENT 直接切到 ACRYLICBLURBEHIND，观感是硬跳变而不是过渡；二是
/// alpha 越拖越高时，混合出来的颜色越来越接近纯白，看起来像"刷白漆"而不是磨砂
/// 变浓。
///
/// 现在的方案：既然模糊半径做不到连续，就不再假装连续，而是把 `blur_level`
/// （0..30，UI 侧滑块以 step=6 吸附）离散量化成 6 个真正有视觉区分度的档位，
/// 每一档手工调过 alpha 上限（最高约 58%，避免完全糊成一面白墙，让模糊后的
/// 背景内容仍隐约透出「磨砂感」而非「纯色填充」），且 tint 颜色改用更中性、更
/// 低亮度的灰调（不再是接近纯白的 `Theme.bg`），从源头减少"发白"观感。
/// 0 档为 ACCENT_DISABLED（完全清透玻璃，不叠加任何 tint），与「不透明度」滑块
/// 的语义保持解耦：不透明度只控制透多少底色，模糊度只控制磨砂浓不浓。
#[cfg(windows)]
fn apply_acrylic_blur_behind(ui: &MainWindow, translucent: bool, blur_level: f32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;

    #[repr(C)]
    struct AccentPolicy {
        accent_state: u32,
        accent_flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }
    #[repr(C)]
    struct WindowCompositionAttribData {
        attrib: u32,
        p_data: *mut core::ffi::c_void,
        data_size: usize,
    }

    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

    type SetWindowCompositionAttributeFn =
        unsafe extern "system" fn(HWND, *mut WindowCompositionAttribData) -> i32;

    // SetWindowCompositionAttribute 是非公开 API：虽然 user32.dll 确有导出，
    // 但 Windows SDK 提供的 user32.lib 只收录公开符号，静态 #[link] 声明会在
    // 链接期报「无法解析的外部符号」。故改为运行时通过 GetProcAddress 动态取址
    // （TranslucentTB 等工具的标准做法），并用 OnceLock 缓存避免重复查找。
    fn resolve_set_window_composition_attribute() -> Option<SetWindowCompositionAttributeFn> {
        use std::sync::OnceLock;
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        static CACHED: OnceLock<usize> = OnceLock::new();
        let addr = *CACHED.get_or_init(|| unsafe {
            let module_name: Vec<u16> = "user32.dll\0".encode_utf16().collect();
            let module = GetModuleHandleW(module_name.as_ptr());
            if module.is_null() {
                return 0;
            }
            let proc_name = b"SetWindowCompositionAttribute\0";
            GetProcAddress(module, proc_name.as_ptr()).map_or(0, |f| f as usize)
        });
        if addr == 0 {
            None
        } else {
            // SAFETY: 地址来自 GetProcAddress 查得的同名导出函数，签名按官方逆向文档核对。
            Some(unsafe { std::mem::transmute::<usize, SetWindowCompositionAttributeFn>(addr) })
        }
    }

    let theme = ui.global::<Theme>();
    // 中性、偏暗灰调的 tint：刻意避开接近纯白/纯黑的取值，alpha 升高时混合结果
    // 趋向"雾蒙蒙的灰玻璃"而不是"刷白漆/刷黑漆"。
    let (r, g, b): (u32, u32, u32) = if theme.get_dark() {
        (33, 38, 48)
    } else {
        (205, 211, 219)
    };

    // 6 档阶梯（对应 UI 滑块 step=6 吸附到 0/6/12/18/24/30），每档 alpha 手工
    // 调过增量与上限，档与档之间要有肉眼可辨的差异，同时上限（148/255≈58%）
    // 留足透光度，避免糊成一面实色墙。
    const TIER_ALPHA: [u32; 6] = [0, 34, 62, 90, 118, 148];

    let (state, alpha) = if !translucent {
        (ACCENT_DISABLED, 0u32)
    } else {
        let tier = ((blur_level / 6.0).round() as i32).clamp(0, 5) as usize;
        if tier == 0 {
            // 完全清透：不叠加任何 tint，纯粹靠「不透明度」滑块控制底色透光。
            (ACCENT_DISABLED, 0u32)
        } else {
            (ACCENT_ENABLE_ACRYLICBLURBEHIND, TIER_ALPHA[tier])
        }
    };
    let gradient_color = (alpha << 24) | (b << 16) | (g << 8) | r;

    let Some(set_window_composition_attribute) = resolve_set_window_composition_attribute() else {
        return;
    };

    ui.window().with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd = isize::from(h.hwnd) as HWND;
            let mut policy = AccentPolicy {
                accent_state: state,
                accent_flags: 0,
                gradient_color,
                animation_id: 0,
            };
            let mut data = WindowCompositionAttribData {
                attrib: WCA_ACCENT_POLICY,
                p_data: &mut policy as *mut _ as *mut core::ffi::c_void,
                data_size: std::mem::size_of::<AccentPolicy>(),
            };
            unsafe {
                set_window_composition_attribute(hwnd, &mut data);
            }
        }
    });
}

/// 剥离 WS_SYSMENU 以消除 DWM 自绘的 Windows 11 原生标题栏按钮（最小化/最大化/关闭），
/// 避免与本程序自绘按钮重叠成「两套」。保留 WS_MINIMIZEBOX | WS_MAXIMIZEBOX 不动——
/// 故 Aero Snap（贴边/Win+方向）与任务栏最小化动画仍正常；自绘按钮走 winit 的
/// set_minimized/set_maximized/hide，不依赖 WS_SYSMENU，功能不受影响。
///
/// 需在任何会重算非客户区的操作之后调用：启用亚克力（延伸边框）、以及最大化/还原切换
/// （winit set_maximized 会触发 FRAMECHANGED，使原生按钮重现）。
#[cfg(windows)]
fn strip_native_caption_buttons(ui: &MainWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_SYSMENU,
    };

    ui.window().with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd = isize::from(h.hwnd) as HWND;
            unsafe {
                let mut style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                style &= !(WS_SYSMENU as isize);
                SetWindowLongPtrW(hwnd, GWL_STYLE, style);
                // 通知系统样式变更并重算非客户区，使原生按钮立即消失
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
            }
        }
    });
}

/// 通过 DWM 让无边框窗口呈现 Windows 11 圆角
#[cfg(windows)]
fn apply_window_round_corners(ui: &MainWindow) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    ui.window().with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let hwnd = isize::from(h.hwnd) as HWND;
            let pref: u32 = DWMWCP_ROUND as u32;
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    &pref as *const u32 as *const core::ffi::c_void,
                    std::mem::size_of::<u32>() as u32,
                );
            }
        }
    });
}

/// 打开 Windows 原生 ChooseColor 取色板对话框（模态）。
/// `initial` 为初始选中颜色，`hwnd` 为父窗口句柄。
/// 用户确认返回 Some(Color)，取消返回 None。
#[cfg(windows)]
fn open_color_picker(initial: slint::Color, hwnd: isize) -> Option<slint::Color> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
    };

    // 16 个自定义颜色槽（ChooseColor 需要，初始化为白）
    let mut cust_colors: [u32; 16] = [0xFFFFFFu32; 16];
    // COLORREF 格式: 0x00BBGGRR（与 RGBA 顺序相反）
    let initial_rgb: u32 =
        (initial.red() as u32) | ((initial.green() as u32) << 8) | ((initial.blue() as u32) << 16);

    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd as *mut core::ffi::c_void,
        hInstance: std::ptr::null_mut(),
        rgbResult: initial_rgb,
        lpCustColors: cust_colors.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        lCustData: 0,
        lpfnHook: None,
        lpTemplateName: std::ptr::null(),
    };

    unsafe {
        if ChooseColorW(&mut cc) != 0 {
            let r = (cc.rgbResult & 0xFF) as u8;
            let g = ((cc.rgbResult >> 8) & 0xFF) as u8;
            let b = ((cc.rgbResult >> 16) & 0xFF) as u8;
            Some(slint::Color::from_rgb_u8(r, g, b))
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
fn open_color_picker(_initial: slint::Color, _hwnd: isize) -> Option<slint::Color> {
    None
}

#[cfg(windows)]
fn set_window_icon(ui: &MainWindow) {
    const ICON_PNG: &[u8] = include_bytes!("../icon.png");
    let load = || -> Option<(Vec<u8>, u32, u32)> {
        let decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).ok()?;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
            png::ColorType::Rgb => buf[..info.buffer_size()]
                .chunks(3)
                .flat_map(|c| [c[0], c[1], c[2], 255u8])
                .collect(),
            _ => return None,
        };
        Some((rgba, info.width, info.height))
    };
    let Some((rgba, w, h)) = load() else {
        return;
    };
    let Ok(icon) = winit::window::Icon::from_rgba(rgba, w, h) else {
        return;
    };
    ui.window().with_winit_window(move |winit_window| {
        winit_window.set_window_icon(Some(icon));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_this_pc_setting_resolves_to_virtual_root() {
        assert_eq!(startup_path("this-pc"), PathBuf::from("this-pc://"));
    }

    #[test]
    fn startup_quick_and_last_use_home_fallback() {
        let home = home_start_path();
        assert_eq!(startup_path("quick"), home);
        assert_eq!(startup_path("last"), home_start_path());
        assert_eq!(startup_path("unknown"), home_start_path());
    }
}

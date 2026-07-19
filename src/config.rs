//! 应用配置持久化：布局尺寸与文件标签
//! 存储位置：%APPDATA%/FerroxPlorer/config.toml

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 三栏宽度与详细视图列宽（单位：逻辑像素）
#[derive(Serialize, Deserialize, Clone)]
pub struct Layout {
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    #[serde(default = "default_details_w")]
    pub details_w: f32,
    #[serde(default = "default_col_modified")]
    pub col_modified: f32,
    #[serde(default = "default_col_kind")]
    pub col_kind: f32,
    #[serde(default = "default_col_size")]
    pub col_size: f32,
    /// 详细视图「固定块」总宽（名称列与三个固定列的分界，拖名称列右缘调整）
    #[serde(default = "default_col_block")]
    pub col_block: f32,
    /// 详细视图三个固定列的显示槽位（拖拽表头排序后持久化；0..2 互不重复）
    #[serde(default)]
    pub col_modified_ord: i32,
    #[serde(default = "d_ord_one")]
    pub col_kind_ord: i32,
    #[serde(default = "d_ord_two")]
    pub col_size_ord: i32,
    /// 双面板左侧占比（0.15..0.85），拖拽中间分隔条调整后持久化
    #[serde(default = "default_dual_ratio")]
    pub dual_ratio: f32,
    /// 主窗口几何（物理像素）：关闭时保存、下次启动恢复。
    /// win_w<=0 表示尚未记录过（首启用默认尺寸）。
    #[serde(default)]
    pub win_x: i32,
    #[serde(default)]
    pub win_y: i32,
    #[serde(default)]
    pub win_w: i32,
    #[serde(default)]
    pub win_h: i32,
    /// 关闭时是否处于最大化（恢复时仅置最大化标志，不覆盖记录的常规尺寸）
    #[serde(default)]
    pub win_maximized: bool,
}

fn default_sidebar_w() -> f32 {
    232.0
}
fn default_details_w() -> f32 {
    292.0
}
fn default_col_modified() -> f32 {
    150.0
}
fn default_col_kind() -> f32 {
    96.0
}
fn default_col_size() -> f32 {
    84.0
}
fn default_col_block() -> f32 {
    484.0
}
fn d_ord_one() -> i32 {
    1
}
fn d_ord_two() -> i32 {
    2
}
fn default_dual_ratio() -> f32 {
    0.5
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_w: default_sidebar_w(),
            details_w: default_details_w(),
            col_modified: default_col_modified(),
            col_kind: default_col_kind(),
            col_size: default_col_size(),
            col_block: default_col_block(),
            col_modified_ord: 0,
            col_kind_ord: 1,
            col_size_ord: 2,
            dual_ratio: default_dual_ratio(),
            win_x: 0,
            win_y: 0,
            win_w: 0,
            win_h: 0,
            win_maximized: false,
        }
    }
}

// ── 用户可调设置项的默认值函数（serde 缺省回退用）──
fn d_true() -> bool {
    true
}
fn d_opacity() -> f32 {
    0.85
}
fn d_icon_scale() -> f32 {
    1.0
}
fn d_theme_mode() -> String {
    "system".into()
}
fn d_accent() -> String {
    "blue".into()
}
fn d_accent_custom() -> String {
    "#0078d4".into()
}
fn d_icon_source() -> String {
    "system".into()
}
fn d_language() -> String {
    "zh-CN".into()
}
fn d_startup_open() -> String {
    "this-pc".into()
}
fn d_default_view() -> String {
    "list".into()
}
fn d_default_sort() -> String {
    "name".into()
}
fn d_new_tab_location() -> String {
    "quick".into()
}
fn d_split_ratio() -> String {
    "50".into()
}
fn d_index_location() -> String {
    "user".into()
}

/// 用户可调设置：对照《开发文档.md》5.11 设置界面
/// 全部字段带 serde 缺省，旧配置文件缺字段时自动回退，向前兼容。
#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    // ── 常规 ──
    #[serde(default)]
    pub launch_on_startup: bool,
    #[serde(default = "d_startup_open")]
    pub startup_open: String,
    #[serde(default = "d_true")]
    pub single_click_open: bool,
    /// 将本应用设为默认文件管理器（HKCU 注册表接管文件夹打开与 Win+E）
    #[serde(default)]
    pub default_file_manager: bool,
    #[serde(default = "d_language")]
    pub language: String,

    // ── 外观 ──
    #[serde(default = "d_theme_mode")]
    pub theme_mode: String, // system | light | dark
    #[serde(default = "d_accent")]
    pub accent: String, // blue | purple | green | red | orange | custom
    /// 自定义主题色（hex 字符串如 "#ff6600"），仅 accent=="custom" 时使用
    #[serde(default = "d_accent_custom")]
    pub accent_custom: String,
    #[serde(default = "d_icon_source")]
    pub icon_source: String,
    #[serde(default)]
    pub compact_mode: bool,
    /// 是否启用界面半透明（毛玻璃）效果
    #[serde(default)]
    pub translucent: bool,
    /// 半透明时的整体不透明度，范围 0.30..=1.0
    #[serde(default = "d_opacity")]
    pub opacity: f32,
    /// 磨砂（毛玻璃）强度，范围 0..=30。
    /// 真正的高斯模糊由 Windows 亚克力系统背景（DWM）提供，但模糊半径系统固定、不可调；
    /// 故本值控制根底层透出度——数值越大根底层越透明，透出越多 DWM 真实磨砂，视觉上越
    /// "模糊"（而非旧实现那样叠一层白色遮罩）。blur=0 时根底层不透明度 = opacity，无磨砂。
    #[serde(default)]
    pub blur: f32,
    /// 图标缩放比例（Ctrl + 鼠标滚轮调节，列表/网格视图与双面板共用），范围 0.7..=2.0
    #[serde(default = "d_icon_scale")]
    pub icon_scale: f32,

    // ── 文件夹 ──
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "d_true")]
    pub show_extensions: bool,
    #[serde(default)]
    pub show_protected: bool,
    #[serde(default)]
    pub calc_folder_size: bool,
    #[serde(default = "d_true")]
    pub folders_first: bool,
    #[serde(default = "d_default_view")]
    pub default_view: String, // list | grid | dual
    #[serde(default = "d_default_sort")]
    pub default_sort: String, // name | size | modified | kind

    // ── 标签页与面板 ──
    #[serde(default = "d_true")]
    pub restore_tabs: bool,
    #[serde(default)]
    pub exit_on_last_tab: bool,
    #[serde(default = "d_new_tab_location")]
    pub new_tab_location: String,
    #[serde(default = "d_true")]
    pub show_details_default: bool,
    #[serde(default)]
    pub dual_pane_default: bool,
    #[serde(default = "d_split_ratio")]
    pub split_ratio: String,

    // ── 搜索与索引 ──
    #[serde(default = "d_true")]
    pub live_filter: bool,
    #[serde(default)]
    pub search_subfolders: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub background_index: bool,
    #[serde(default = "d_index_location")]
    pub index_location: String,

    // ── 右键菜单 ──
    /// 是否使用 Windows 系统原生右键菜单（true）而非应用自定义菜单（false）。
    /// 默认使用系统菜单（含第三方注册项，与资源管理器一致）。
    #[serde(default = "d_true")]
    pub context_menu_system: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            launch_on_startup: false,
            startup_open: d_startup_open(),
            single_click_open: true,
            default_file_manager: false,
            language: d_language(),
            theme_mode: d_theme_mode(),
            accent: d_accent(),
            accent_custom: d_accent_custom(),
            icon_source: d_icon_source(),
            compact_mode: false,
            translucent: false,
            opacity: d_opacity(),
            blur: 0.0,
            icon_scale: d_icon_scale(),
            show_hidden: false,
            show_extensions: true,
            show_protected: false,
            calc_folder_size: false,
            folders_first: true,
            default_view: d_default_view(),
            default_sort: d_default_sort(),
            restore_tabs: true,
            exit_on_last_tab: false,
            new_tab_location: d_new_tab_location(),
            show_details_default: true,
            dual_pane_default: false,
            split_ratio: d_split_ratio(),
            live_filter: true,
            search_subfolders: false,
            case_sensitive: false,
            background_index: false,
            index_location: d_index_location(),
            context_menu_system: true,
        }
    }
}

/// 用户保存的网络位置连接（SMB 挂载等）
#[derive(Clone, Serialize, Deserialize)]
pub struct NetworkLocation {
    pub name: String,          // 显示名
    pub server: String,        // SMB: \\server\share
    pub kind: String,          // "smb"（"webdav" 预留）
    pub drive: Option<String>, // 挂载盘符（如 "Z:"）；未挂载为 None
}

/// 完整应用配置
#[derive(Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub layout: Layout,
    /// 用户可调设置
    #[serde(default)]
    pub settings: Settings,
    /// 绝对路径 -> 标签键集合（"important" / "archive" / "done"）
    #[serde(default)]
    pub tags: BTreeMap<String, Vec<String>>,
    /// 用户保存的网络位置连接
    #[serde(default)]
    pub network_locations: Vec<NetworkLocation>,
}

/// 配置文件完整路径
fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("FerroxPlorer").join("config.toml"))
}

impl AppConfig {
    /// 读取配置；文件缺失或解析失败时返回默认值
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 序列化写回磁盘（自动创建父目录）
    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    /// 切换某路径的某标签：已有则移除，没有则添加。返回切换后是否含该标签
    pub fn toggle_tag(&mut self, path: &str, tag: &str) -> bool {
        let list = self.tags.entry(path.to_string()).or_default();
        if let Some(pos) = list.iter().position(|t| t == tag) {
            list.remove(pos);
            if list.is_empty() {
                self.tags.remove(path);
            }
            false
        } else {
            list.push(tag.to_string());
            true
        }
    }

    /// 某路径是否含指定标签
    pub fn has_tag(&self, path: &str, tag: &str) -> bool {
        self.tags
            .get(path)
            .map(|v| v.iter().any(|t| t == tag))
            .unwrap_or(false)
    }

    /// 移除某路径的全部标签（用于清理已失效路径）
    pub fn remove_path_tags(&mut self, path: &str) {
        self.tags.remove(path);
    }

    /// 拥有指定标签的全部路径
    pub fn paths_with_tag(&self, tag: &str) -> Vec<String> {
        self.tags
            .iter()
            .filter(|(_, tags)| tags.iter().any(|t| t == tag))
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// 拥有指定标签的路径数量
    pub fn count(&self, tag: &str) -> usize {
        self.tags
            .values()
            .filter(|tags| tags.iter().any(|t| t == tag))
            .count()
    }
}

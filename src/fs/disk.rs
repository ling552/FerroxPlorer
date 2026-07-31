//! 磁盘信息：枚举驱动器并查询容量（Windows 真实查询）

use super::metadata::Entry;
use std::sync::{Mutex, OnceLock};

fn disk_cache() -> &'static Mutex<Vec<DiskInfo>> {
    static CACHE: OnceLock<Mutex<Vec<DiskInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// 返回最近一次后台/显式枚举得到的磁盘快照，不访问任何盘符。
pub fn cached_disks() -> Vec<DiskInfo> {
    disk_cache().lock().map(|v| v.clone()).unwrap_or_default()
}

/// 从缓存快照查询单个盘符，不触发设备访问；交互热路径（选中/状态栏）专用。
pub fn cached_disk_info_of(letter: char) -> Option<DiskInfo> {
    let letter = letter.to_ascii_uppercase().to_string();
    disk_cache()
        .lock()
        .ok()?
        .iter()
        .find(|d| d.letter.eq_ignore_ascii_case(&letter))
        .cloned()
}

fn replace_cache(disks: &[DiskInfo]) {
    if let Ok(mut cache) = disk_cache().lock() {
        *cache = disks.to_vec();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Optical,
    Network,
    Ram,
    Unknown,
}

impl DriveKind {
    pub fn label(&self) -> &'static str {
        match self {
            DriveKind::Fixed => "本地磁盘",
            DriveKind::Removable => "可移动磁盘",
            DriveKind::Optical => "光驱",
            DriveKind::Network => "网络驱动器",
            DriveKind::Ram => "RAM 磁盘",
            DriveKind::Unknown => "驱动器",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiskInfo {
    pub letter: String,
    pub name: String,
    pub root: String,
    pub total: u64,
    pub free: u64,
    pub kind: DriveKind,
}

impl DiskInfo {
    /// 已用比例 0..1
    pub fn used_ratio(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            ((self.total - self.free) as f64 / self.total as f64) as f32
        }
    }

    pub fn to_entry(&self) -> Entry {
        Entry {
            name: self.name.clone(),
            path: self.root.clone(),
            is_dir: true,
            size_bytes: self.total,
            modified_ts: 0,
            kind: self.kind.label().to_string(),
            icon_label: self.letter.clone(),
            icon_class: "drive".into(),
        }
    }
}

pub fn disk_entries() -> Vec<Entry> {
    list_disks().into_iter().map(|d| d.to_entry()).collect()
}

/// 查询单个盘符的驱动器信息（类型/容量/卷标）。保留给非交互后台任务使用。
#[cfg(windows)]
#[allow(dead_code)]
pub fn disk_info_of(letter: char) -> Option<DiskInfo> {
    let letter = letter.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    query_disk(letter)
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub fn disk_info_of(_letter: char) -> Option<DiskInfo> {
    None
}

#[cfg(windows)]
pub fn list_disks() -> Vec<DiskInfo> {
    use windows_sys::Win32::Storage::FileSystem::GetLogicalDrives;

    let mut disks = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        if let Some(d) = query_disk(letter) {
            disks.push(d);
        }
    }
    replace_cache(&disks);
    disks
}

/// 查询单个盘符的完整信息（list_disks 与 disk_info_of 共用）
#[cfg(windows)]
fn query_disk(letter: char) -> Option<DiskInfo> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW,
    };

    let root = format!("{}:\\", letter);
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();

    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    // DRIVE_NO_ROOT_DIR(1)：盘符不存在（disk_info_of 传入任意字母时的防御）
    if drive_type == 1 {
        return None;
    }
    let kind = match drive_type {
        3 => DriveKind::Fixed,
        2 => DriveKind::Removable,
        5 => DriveKind::Optical,
        4 => DriveKind::Network,
        6 => DriveKind::Ram,
        _ => DriveKind::Unknown,
    };

    let mut free_avail: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_avail, &mut total, &mut total_free) };
    if ok == 0 {
        free_avail = 0;
        total = 0;
    }

    let mut volume_name = [0u16; 260];
    let volume_ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    } != 0;
    let label = if volume_ok {
        let len = volume_name.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&volume_name[..len])
            .trim()
            .to_string()
    } else {
        String::new()
    };

    let fallback = match kind {
        DriveKind::Fixed if letter == 'C' => "Windows".to_string(),
        DriveKind::Fixed => "本地磁盘".to_string(),
        _ => kind.label().to_string(),
    };
    let base_name = if label.is_empty() { fallback } else { label };
    let name = format!("{} ({}:)", base_name, letter);

    Some(DiskInfo {
        letter: letter.to_string(),
        name,
        root,
        total,
        free: free_avail,
        kind,
    })
}

#[cfg(not(windows))]
pub fn list_disks() -> Vec<DiskInfo> {
    vec![DiskInfo {
        letter: "/".into(),
        name: "根文件系统".into(),
        root: "/".into(),
        total: 0,
        free: 0,
        kind: DriveKind::Fixed,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_info_converts_to_this_pc_entry() {
        let disk = DiskInfo {
            letter: "D".into(),
            name: "Data (D:)".into(),
            root: "D:\\".into(),
            total: 1024,
            free: 256,
            kind: DriveKind::Fixed,
        };

        let entry = disk.to_entry();

        assert_eq!(entry.name, "Data (D:)");
        assert_eq!(entry.path, "D:\\");
        assert!(entry.is_dir);
        assert_eq!(entry.size_bytes, 1024);
        assert_eq!(entry.kind, "本地磁盘");
        assert_eq!(entry.icon_label, "D");
        assert_eq!(entry.icon_class, "drive");
    }

    #[test]
    fn unavailable_drive_keeps_zero_usage() {
        let disk = DiskInfo {
            letter: "I".into(),
            name: "光驱 (I:)".into(),
            root: "I:\\".into(),
            total: 0,
            free: 0,
            kind: DriveKind::Optical,
        };

        assert_eq!(disk.used_ratio(), 0.0);
        assert_eq!(disk.to_entry().kind, "光驱");
    }
}

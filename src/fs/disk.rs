//! 磁盘信息：枚举驱动器并查询容量（Windows 真实查询）

use super::metadata::Entry;

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

#[cfg(windows)]
pub fn list_disks() -> Vec<DiskInfo> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    let mut disks = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{}:\\", letter);
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();

        let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
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
        let ok = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut free_avail, &mut total, &mut total_free)
        };
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

        disks.push(DiskInfo {
            letter: letter.to_string(),
            name,
            root,
            total,
            free: free_avail,
            kind,
        });
    }
    disks
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

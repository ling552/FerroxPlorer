//! 便携设备枚举与浏览：WPD（Windows Portable Devices）COM 接口
//! - list_devices：枚举手机/平板/相机（IPortableDeviceManager），自动排除第三方命名空间扩展
//! - list_content：进入设备内部，枚举某对象下的子对象（文件夹/文件），实现"像资源管理器一样浏览"
//! - copy_to_temp：把设备上的文件复制到临时目录，供系统默认程序打开
//! - parent_path：取某对象的父对象虚拟路径，供"向上"导航
//!
//! 虚拟路径编码：
//!   device://<deviceId>                 设备根（枚举 WPD_DEVICE_OBJECT_ID="DEVICE" 的子对象）
//!   device://<deviceId>\u{1}<objectId>  设备内部某对象（\u{1} SOH 作分隔，因对象 ID 可含任意字符）

use super::metadata::Entry;

/// 设备路径中 deviceId 与 objectId 的分隔符（SOH，对象 ID 不会包含该控制字符）
pub const SEP: char = '\u{1}';

/// 协议判定缓存：device_id → (是否 USB 大容量存储, 负缓存过期时间)。
/// 打开设备读协议有数十毫秒开销，而枚举随 1.5s 热插拔轮询在 UI 线程高频调用：
/// 成功判定永久缓存（同一设备协议不变，过期时间为 None）；读取失败（设备忙/
/// 锁屏拒绝）记短期负缓存（30s 内不重试），避免慢设备让界面每轮轮询都卡顿。
#[cfg(windows)]
static MSC_CACHE: std::sync::Mutex<
    Option<std::collections::HashMap<String, (bool, Option<std::time::Instant>)>>,
> = std::sync::Mutex::new(None);

/// 设备友好名缓存：device_id → 名称（来自最近一次枚举）。
/// 标签页标题 / 面包屑经 virtualfs::friendly_title 查询，避免显示原始设备 ID。
#[cfg(windows)]
static NAME_CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
    std::sync::Mutex::new(None);

/// 查询设备友好名（最近一次枚举的缓存）。未知设备返回 None。
pub fn friendly_name(device_id: &str) -> Option<String> {
    #[cfg(windows)]
    {
        NAME_CACHE.lock().ok()?.as_ref()?.get(device_id).cloned()
    }
    #[cfg(not(windows))]
    {
        let _ = device_id;
        None
    }
}

pub fn list_devices() -> Vec<Entry> {
    #[cfg(windows)]
    {
        list_devices_win().unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// 解析 device:// 虚拟路径，返回 (deviceId, objectId)。
/// 无 objectId 时默认设备根对象 "DEVICE"。返回 None 表示不是合法 device:// 路径。
pub fn parse(vpath: &str) -> Option<(String, String)> {
    let rest = vpath.strip_prefix("device://")?;
    match rest.split_once(SEP) {
        Some((dev, obj)) => Some((dev.to_string(), obj.to_string())),
        None => Some((rest.to_string(), "DEVICE".to_string())),
    }
}

/// 列出设备内某对象下的子项（文件夹/文件）。失败或非 device:// 路径返回空。
pub fn list_content(vpath: &str) -> Vec<Entry> {
    #[cfg(windows)]
    {
        list_content_win(vpath).unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        let _ = vpath;
        Vec::new()
    }
}

/// 把设备上的文件复制到临时目录，返回本地临时文件路径（供系统默认程序打开）。
pub fn copy_to_temp(vpath: &str) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        copy_to_temp_win(vpath).ok()
    }
    #[cfg(not(windows))]
    {
        let _ = vpath;
        None
    }
}

/// 取某对象的父对象虚拟路径，供"向上"导航。已在设备根（objectId=="DEVICE"）时返回 None。
pub fn parent_path(vpath: &str) -> Option<String> {
    #[cfg(windows)]
    {
        parent_path_win(vpath)
    }
    #[cfg(not(windows))]
    {
        let _ = vpath;
        None
    }
}

#[cfg(windows)]
fn pwstr_read(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len)).to_string()
    }
}

#[cfg(windows)]
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn list_devices_win() -> windows::core::Result<Vec<Entry>> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Devices::PortableDevices::{IPortableDeviceManager, PortableDeviceManager};
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};

    let manager: IPortableDeviceManager =
        unsafe { CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)? };

    // WPD manager 在进程内缓存设备列表：不刷新的话，已拔出的设备会一直被枚举出来
    // （表现为「拔下设备后侧边栏依旧显示」），必须先显式刷新
    let _ = unsafe { manager.RefreshDeviceList() };

    let mut count = 0u32;
    unsafe { manager.GetDevices(std::ptr::null_mut(), &mut count)? };
    if count == 0 {
        return Ok(Vec::new());
    }

    let mut ids: Vec<PWSTR> = vec![PWSTR(std::ptr::null_mut()); count as usize];
    unsafe { manager.GetDevices(ids.as_mut_ptr(), &mut count)? };

    let mut entries = Vec::new();
    for id in ids.iter().take(count as usize) {
        let id_str = pwstr_read(id.0);
        unsafe { CoTaskMemFree(Some(id.0.cast())) };

        // 排除 USB 大容量存储（协议 "MSC"）：它们已作为驱动器出现在「此电脑」，
        // WPD 影子设备的内容只是一个盘符对象，进入后等于绕道浏览本地磁盘
        if is_mass_storage(&id_str) {
            continue;
        }

        let id_wide = to_wide(&id_str);
        let pc_id = PCWSTR(id_wide.as_ptr());

        let mut name_len = 0u32;
        let _ = unsafe {
            manager.GetDeviceFriendlyName(pc_id, PWSTR(std::ptr::null_mut()), &mut name_len)
        };
        let name = if name_len > 0 {
            let mut buf: Vec<u16> = vec![0u16; name_len as usize];
            let ok = unsafe {
                manager.GetDeviceFriendlyName(pc_id, PWSTR(buf.as_mut_ptr()), &mut name_len)
            }
            .is_ok();
            if ok {
                pwstr_read(buf.as_ptr())
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        // 取不到友好名时退回通用名称（原始设备 ID 太长且无可读性，不再直接展示）
        let name = if name.trim().is_empty() {
            "便携设备".to_string()
        } else {
            name
        };

        // 写入友好名缓存，供标签页标题 / 面包屑查询
        if let Ok(mut cache) = NAME_CACHE.lock() {
            cache
                .get_or_insert_with(std::collections::HashMap::new)
                .insert(id_str.clone(), name.clone());
        }

        entries.push(Entry {
            name,
            path: format!("device://{}", id_str),
            is_dir: true,
            size_bytes: 0,
            modified_ts: 0,
            kind: "便携设备".to_string(),
            icon_label: String::new(),
            icon_class: "device".into(),
        });
    }

    Ok(entries)
}

/// 读取设备协议字符串（如 "MTP: 1.00" / "PTP: 1.00" / "MSC:"）。失败返回 None。
#[cfg(windows)]
fn read_protocol(device_id: &str) -> Option<String> {
    use windows::Win32::Devices::PortableDevices::{
        IPortableDeviceKeyCollection, PortableDeviceKeyCollection, WPD_DEVICE_OBJECT_ID,
        WPD_DEVICE_PROTOCOL,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let device = open_device(device_id).ok()?;
    let content = unsafe { device.Content() }.ok()?;
    let props = unsafe { content.Properties() }.ok()?;
    let keys: IPortableDeviceKeyCollection =
        unsafe { CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER) }
            .ok()?;
    unsafe { keys.Add(&WPD_DEVICE_PROTOCOL) }.ok()?;
    let vals = unsafe { props.GetValues(WPD_DEVICE_OBJECT_ID, &keys) }.ok()?;
    let s = get_string(&vals, &WPD_DEVICE_PROTOCOL);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 判定设备是否为 USB 大容量存储（协议以 "MSC" 开头）。
/// 成功判定永久缓存；读不到协议（设备忙/刚拔出）保守视为便携设备并记 30s 负缓存。
#[cfg(windows)]
fn is_mass_storage(device_id: &str) -> bool {
    if let Ok(cache) = MSC_CACHE.lock() {
        if let Some((v, expires)) = cache.as_ref().and_then(|m| m.get(device_id)) {
            match expires {
                None => return *v,
                Some(t) if std::time::Instant::now() < *t => return *v,
                Some(_) => {} // 负缓存过期，重新判定
            }
        }
    }
    let (msc, expires) = match read_protocol(device_id) {
        Some(p) => (p.trim_start().to_ascii_uppercase().starts_with("MSC"), None),
        None => (
            false,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(30)),
        ),
    };
    if let Ok(mut cache) = MSC_CACHE.lock() {
        cache
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(device_id.to_string(), (msc, expires));
    }
    msc
}

/// 打开设备并返回 IPortableDevice。设置最小客户端信息（WPD_CLIENT_NAME）。
#[cfg(windows)]
fn open_device(
    device_id: &str,
) -> windows::core::Result<windows::Win32::Devices::PortableDevices::IPortableDevice> {
    use windows::core::PCWSTR;
    use windows::Win32::Devices::PortableDevices::{
        IPortableDevice, IPortableDeviceValues, PortableDevice, PortableDeviceValues,
        WPD_CLIENT_NAME,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let client: IPortableDeviceValues =
        unsafe { CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)? };
    let app_name = to_wide("FerroxPlorer");
    unsafe { client.SetStringValue(&WPD_CLIENT_NAME, PCWSTR(app_name.as_ptr()))? };

    let device: IPortableDevice =
        unsafe { CoCreateInstance(&PortableDevice, None, CLSCTX_INPROC_SERVER)? };
    let dev_wide = to_wide(device_id);
    unsafe { device.Open(PCWSTR(dev_wide.as_ptr()), &client)? };
    Ok(device)
}

/// 读取对象的字符串属性，自动释放 COM 字符串。
#[cfg(windows)]
fn get_string(
    vals: &windows::Win32::Devices::PortableDevices::IPortableDeviceValues,
    key: &windows::Win32::Foundation::PROPERTYKEY,
) -> String {
    use windows::Win32::System::Com::CoTaskMemFree;
    unsafe {
        match vals.GetStringValue(key) {
            Ok(p) => {
                let s = pwstr_read(p.0);
                CoTaskMemFree(Some(p.0.cast()));
                s
            }
            Err(_) => String::new(),
        }
    }
}

#[cfg(windows)]
fn list_content_win(vpath: &str) -> windows::core::Result<Vec<Entry>> {
    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::Devices::PortableDevices::{
        IPortableDeviceKeyCollection, PortableDeviceKeyCollection, WPD_CONTENT_TYPE_FOLDER,
        WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_OBJECT_CONTENT_TYPE, WPD_OBJECT_DATE_MODIFIED,
        WPD_OBJECT_NAME, WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_OBJECT_SIZE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VT_DATE;

    let (device_id, parent_obj) = match parse(vpath) {
        Some(v) => v,
        None => return Ok(Vec::new()),
    };

    let device = open_device(&device_id)?;
    let content = unsafe { device.Content()? };
    let props = unsafe { content.Properties()? };

    // 只取需要的属性，减少跨进程往返
    let keys: IPortableDeviceKeyCollection =
        unsafe { CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        keys.Add(&WPD_OBJECT_NAME)?;
        keys.Add(&WPD_OBJECT_ORIGINAL_FILE_NAME)?;
        keys.Add(&WPD_OBJECT_CONTENT_TYPE)?;
        keys.Add(&WPD_OBJECT_SIZE)?;
        keys.Add(&WPD_OBJECT_DATE_MODIFIED)?;
    }

    let parent_wide = to_wide(&parent_obj);
    let enumr = unsafe { content.EnumObjects(0, PCWSTR(parent_wide.as_ptr()), None)? };

    let mut entries = Vec::new();
    let mut objids: [windows::core::PWSTR; 32] = [windows::core::PWSTR(std::ptr::null_mut()); 32];
    loop {
        let mut fetched = 0u32;
        let _hr = unsafe { enumr.Next(&mut objids, &mut fetched) };
        if fetched == 0 {
            break;
        }
        for slot in objids.iter().take(fetched as usize) {
            let oid_ptr = slot.0;
            let oid_str = pwstr_read(oid_ptr);
            let oid_wide = to_wide(&oid_str);

            if let Ok(vals) = unsafe { props.GetValues(PCWSTR(oid_wide.as_ptr()), &keys) } {
                let orig = get_string(&vals, &WPD_OBJECT_ORIGINAL_FILE_NAME);
                let name = if !orig.is_empty() {
                    orig
                } else {
                    get_string(&vals, &WPD_OBJECT_NAME)
                };
                let ctype =
                    unsafe { vals.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE) }.unwrap_or_default();
                let is_dir =
                    ctype == WPD_CONTENT_TYPE_FOLDER || ctype == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT;
                let size = if is_dir {
                    0
                } else {
                    unsafe { vals.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE) }.unwrap_or(0)
                };
                // 修改日期：WPD 以 OLE DATE（自 1899-12-30 起的天数，本地墙钟时间）返回。
                // 先换算成"本地秒"，再按该时间点当时生效的时区规则（含夏令时）转 unix 秒
                // （显示层会再做一次 UTC→本地转换，不校正会偏移一个时区）。
                // 缺失时保持 0（UI 对 0 显示为空而非 1970）。
                let modified_ts = unsafe { vals.GetValue(&WPD_OBJECT_DATE_MODIFIED) }
                    .ok()
                    .and_then(|mut pv| {
                        let local_secs = unsafe {
                            let inner = &pv.Anonymous.Anonymous;
                            if inner.vt == VT_DATE {
                                // 25569 = 1899-12-30 与 1970-01-01 之间的天数
                                Some(((inner.Anonymous.date - 25569.0) * 86400.0) as i64)
                            } else {
                                None
                            }
                        };
                        // COM 契约：GetValue 返回的 PROPVARIANT 由调用方释放。
                        // VT_DATE 是标量本为 no-op，但不合规驱动可能返回分配型变体
                        unsafe {
                            let _ =
                                windows::Win32::System::Com::StructuredStorage::PropVariantClear(
                                    &mut pv,
                                );
                        }
                        let local_secs = local_secs?;
                        use chrono::TimeZone;
                        let naive = chrono::DateTime::from_timestamp(local_secs, 0)?.naive_utc();
                        chrono::Local
                            .from_local_datetime(&naive)
                            .earliest()
                            .map(|dt| dt.timestamp())
                    })
                    .filter(|&ts| ts > 0)
                    .unwrap_or(0);

                let (icon_class, icon_label, kind) =
                    super::metadata::classify(Path::new(&name), is_dir);
                entries.push(Entry {
                    name,
                    path: format!("device://{}{}{}", device_id, SEP, oid_str),
                    is_dir,
                    size_bytes: size,
                    modified_ts,
                    kind,
                    icon_label,
                    icon_class,
                });
            }

            unsafe { CoTaskMemFree(Some(oid_ptr.cast())) };
        }
    }

    Ok(entries)
}

#[cfg(windows)]
fn copy_to_temp_win(vpath: &str) -> windows::core::Result<std::path::PathBuf> {
    use std::io::Write;
    use windows::core::{Error, HRESULT, PCWSTR};
    use windows::Win32::Devices::PortableDevices::{
        IPortableDeviceKeyCollection, PortableDeviceKeyCollection, WPD_OBJECT_NAME,
        WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_RESOURCE_DEFAULT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, IStream, CLSCTX_INPROC_SERVER, COINIT_DISABLE_OLE1DDE,
        COINIT_MULTITHREADED, STGM_READ,
    };

    // 本函数在后台线程被调用（open_device_file 的 spawn），必须先初始化本线程 COM；
    // 已初始化时返回 S_FALSE，忽略即可（与 thumbnail.rs 后台线程一致）
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE);
    }

    let (device_id, obj) =
        parse(vpath).ok_or_else(|| Error::new(HRESULT(-1), "非 device:// 路径"))?;
    if obj == "DEVICE" {
        return Err(Error::new(HRESULT(-1), "设备根不可作为文件打开"));
    }

    let device = open_device(&device_id)?;
    let content = unsafe { device.Content()? };
    let props = unsafe { content.Properties()? };

    // 取原始文件名用于临时文件命名
    let keys: IPortableDeviceKeyCollection =
        unsafe { CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)? };
    unsafe {
        keys.Add(&WPD_OBJECT_ORIGINAL_FILE_NAME)?;
        keys.Add(&WPD_OBJECT_NAME)?;
    }
    let obj_wide = to_wide(&obj);
    let fname = match unsafe { props.GetValues(PCWSTR(obj_wide.as_ptr()), &keys) } {
        Ok(vals) => {
            let orig = get_string(&vals, &WPD_OBJECT_ORIGINAL_FILE_NAME);
            if !orig.is_empty() {
                orig
            } else {
                get_string(&vals, &WPD_OBJECT_NAME)
            }
        }
        Err(_) => String::new(),
    };
    let fname = sanitize_filename(&fname, &obj);

    // 取默认资源流
    let resources = unsafe { content.Transfer()? };
    let mut optimal = 0u32;
    let mut stream: Option<IStream> = None;
    unsafe {
        resources.GetStream(
            PCWSTR(obj_wide.as_ptr()),
            &WPD_RESOURCE_DEFAULT,
            STGM_READ.0 as u32,
            &mut optimal,
            &mut stream,
        )?
    };
    let stream = stream.ok_or_else(|| Error::new(HRESULT(-1), "无法获取设备文件流"))?;

    let dir = std::env::temp_dir().join("FerroxPlorer_mtp");
    let _ = std::fs::create_dir_all(&dir);
    let out_path = dir.join(&fname);
    let mut file = std::fs::File::create(&out_path)
        .map_err(|e| Error::new(HRESULT(-1), format!("创建临时文件失败: {e}")))?;

    let buf_size = if optimal == 0 {
        256 * 1024
    } else {
        optimal as usize
    };
    let mut buf = vec![0u8; buf_size];
    loop {
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read)) };
        if read > 0 {
            file.write_all(&buf[..read as usize])
                .map_err(|e| Error::new(HRESULT(-1), format!("写入临时文件失败: {e}")))?;
        }
        // S_OK(0) 继续；S_FALSE(1) 表示已到末尾，读完本批后结束；读为 0 也结束
        if read == 0 || hr.0 != 0 {
            break;
        }
    }
    let _ = file.flush();

    Ok(out_path)
}

/// 清洗文件名中的非法字符；为空时用对象 ID 兜底。
#[cfg(windows)]
fn sanitize_filename(name: &str, fallback_obj: &str) -> String {
    let base = if name.trim().is_empty() {
        fallback_obj
    } else {
        name
    };
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    if cleaned.trim().is_empty() {
        "mtp_file".to_string()
    } else {
        cleaned
    }
}

#[cfg(windows)]
fn parent_path_win(vpath: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Devices::PortableDevices::{
        IPortableDeviceKeyCollection, PortableDeviceKeyCollection, WPD_OBJECT_PARENT_ID,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

    let (device_id, obj) = parse(vpath)?;
    if obj == "DEVICE" {
        return None; // 已在设备根
    }

    let device = open_device(&device_id).ok()?;
    let content = unsafe { device.Content().ok()? };
    let props = unsafe { content.Properties().ok()? };

    let keys: IPortableDeviceKeyCollection =
        unsafe { CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER).ok()? };
    unsafe { keys.Add(&WPD_OBJECT_PARENT_ID).ok()? };

    let obj_wide = to_wide(&obj);
    let vals = unsafe { props.GetValues(PCWSTR(obj_wide.as_ptr()), &keys).ok()? };
    let parent = get_string(&vals, &WPD_OBJECT_PARENT_ID);

    if parent.is_empty() || parent == "DEVICE" {
        Some(format!("device://{}", device_id))
    } else {
        Some(format!("device://{}{}{}", device_id, SEP, parent))
    }
}

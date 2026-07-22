//! OLE 拖出：把选中的文件/文件夹拖拽到其他应用（资源管理器复制、浏览器上传、
//! 聊天窗口发送等），与从资源管理器拖出的行为一致。
//!
//! IDataObject 不手工实现：把路径解析为 PIDL 后经 IShellItemArray::BindToHandler
//! (BHID_DataObject) 取得 Shell 提供的数据对象——自带 CF_HDROP 与全部 Shell 剪贴板
//! 格式，目标应用兼容性与资源管理器完全一致。本模块只实现极简 IDropSource
//! （Esc 取消 / 松开左键放下）。
//!
//! 线程前提：winit 创建主窗口时已对 UI 线程 OleInitialize（STA），DoDragDrop
//! 直接调用即可；它自带模态消息循环，阻塞至用户放下或取消（与 TrackPopupMenuEx
//! 系统右键菜单同模式，项目已长期验证可行）。

/// 启动 OLE 拖出（模态，阻塞至拖拽结束）。
/// 返回最终 DROPEFFECT 数值：0 无 / 1 复制 / 2 移动 / 4 链接。
#[cfg(windows)]
pub fn run(paths: &[String]) -> u32 {
    win_impl::run(paths)
}

#[cfg(not(windows))]
pub fn run(_paths: &[String]) -> u32 {
    0
}

#[cfg(windows)]
mod win_impl {
    use windows::core::{implement, PCWSTR};
    use windows::Win32::Foundation::{
        BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
    };
    use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
    use windows::Win32::System::Ole::{
        DoDragDrop, IDropSource, IDropSource_Impl, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK,
        DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };

    /// 极简拖拽源：Esc → 取消；左键松开 → 放下；其余继续
    #[implement(IDropSource)]
    struct DropSource;

    impl IDropSource_Impl for DropSource_Impl {
        fn QueryContinueDrag(
            &self,
            fescapepressed: BOOL,
            grfkeystate: MODIFIERKEYS_FLAGS,
        ) -> windows::core::HRESULT {
            if fescapepressed.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> windows::core::HRESULT {
            // 使用系统默认拖拽光标（复制加号/移动箭头等）
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    pub fn run(paths: &[String]) -> u32 {
        if paths.is_empty() {
            return 0;
        }
        unsafe {
            // 路径 → PIDL（解析失败的逐项跳过）
            let mut pidls: Vec<*const ITEMIDLIST> = Vec::with_capacity(paths.len());
            for p in paths {
                let wide: Vec<u16> = p.encode_utf16().chain(std::iter::once(0)).collect();
                let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_ok()
                    && !pidl.is_null()
                {
                    pidls.push(pidl as *const ITEMIDLIST);
                }
            }
            if pidls.is_empty() {
                return 0;
            }

            // Shell 数据对象（含 CF_HDROP 与全部 Shell 格式）
            let effect = (|| -> windows::core::Result<DROPEFFECT> {
                let array = SHCreateShellItemArrayFromIDLists(&pidls)?;
                let data_obj: IDataObject = array.BindToHandler(None, &BHID_DataObject)?;
                let source: IDropSource = DropSource.into();
                let mut effect = DROPEFFECT_NONE;
                // 模态拖拽循环：阻塞至放下/取消；返回值（取消/放下）不影响 effect 语义
                let _ = DoDragDrop(
                    &data_obj,
                    &source,
                    DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
                    &mut effect,
                );
                Ok(effect)
            })()
            .unwrap_or(DROPEFFECT_NONE);

            // 释放 PIDL
            for pidl in pidls {
                CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
            }
            effect.0
        }
    }
}

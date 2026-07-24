# FerroxPlorer

现代化 Windows 桌面文件管理器,使用 **Rust + Slint** 实现。

[![Platform](https://img.shields.io/badge/platform-Windows-blue)](https://github.com/ling552/FerroxPlorer/releases)
[![License](https://img.shields.io/badge/license-MIT-green)](https://github.com/ling552/FerroxPlorer/blob/main/LICENSE)

![ico](icon.webp)

## 功能特性

- 📁 单/双面板文件浏览,列表与网格视图,标签页
- 🎨 亚克力磨砂质感界面,深浅色主题,多主题色
- 🔍 实时过滤搜索、命令面板、路径补全
- 📦 ZIP / 7z / TAR / tar.gz 归档压缩与解压(后台任务 + 实时进度)
- 🖼️ 系统图标 / 缩略图 / 视频首帧、空格键 Quick Look 快速预览
- 🔧 属性对话框:常规 / 安全(ACL)/ 详细信息(EXIF、音频标签)/ 数字签名
- 🌿 Git 集成:分支显示与文件状态徽章
- ♻️ 回收站、快速访问、网络位置、设备管理
- 🔄 内置更新:从 GitHub Releases 检查并下载更新
- ⌨️ 完整快捷键:复制/剪切/粘贴/撤销/重做/重命名/全选等

## 安装

从 [Releases](../../releases) 页面下载最新的 `FerroxPlorer-Setup-x.y.z.exe` 安装程序。

## 从源码构建

需要 Rust 工具链(stable):

```bash
cargo build --release
```

产物位于 `target/release/ferroxplorer.exe`。

## 许可证

[MIT](LICENSE)

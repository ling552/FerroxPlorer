// 构建脚本：编译 Slint UI，并在 Windows 上从 icon.png 生成 icon.ico 嵌入 exe
fn main() {
    // Slint 主窗口编译。
    // 在 64MB 大栈线程中执行：Slint 编译器对表达式/组件树做深递归，
    // UI 复杂度增长后会撑爆 Windows build script 主线程默认 1MB 栈
    // （STATUS_STACK_OVERFLOW 0xc00000fd）。
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let config =
                slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
            slint_build::compile_with_config("ui/main.slint", config).expect("Slint 主窗口编译失败");
        })
        .expect("创建 Slint 编译线程失败")
        .join()
        .expect("Slint 编译线程崩溃");

    // Windows：以 icon.png 为唯一图标源，构建时生成多尺寸 icon.ico 并嵌入 exe，
    // 作为任务栏 / 标题栏 / 资源管理器中的应用程序图标
    #[cfg(windows)]
    {
        // icon.png 变化时重新构建
        println!("cargo:rerun-if-changed=icon.png");

        match build_ico_from_png() {
            Ok(ico_path) => {
                let mut res = winres::WindowsResource::new();
                res.set_icon(&ico_path.to_string_lossy());
                if let Err(e) = res.compile() {
                    // 图标嵌入失败不应阻断构建，仅打印警告
                    println!("cargo:warning=应用图标嵌入失败：{}", e);
                }
            }
            Err(e) => {
                // 生成 ico 失败同样不阻断构建（运行时仍由 winit 设置窗口图标）
                println!("cargo:warning=从 icon.png 生成 icon.ico 失败：{}", e);
            }
        }
    }
}

// 从 icon.png 解码为 RGBA，按多尺寸缩放后编码为 ICO，写入 OUT_DIR 并返回路径
#[cfg(windows)]
fn build_ico_from_png() -> Result<std::path::PathBuf, String> {
    // 解码 icon.png 为 RGBA 像素
    let file = std::fs::File::open("icon.png").map_err(|e| format!("打开 icon.png 失败：{}", e))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("读取 PNG 信息失败：{}", e))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("解码 PNG 帧失败：{}", e))?;
    let (sw, sh) = (info.width, info.height);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255u8])
            .collect(),
        other => return Err(format!("不支持的 PNG 颜色类型：{:?}", other)),
    };

    // 组装多尺寸 ICO，覆盖资源管理器常用的小/中/大图标尺寸
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in &[16u32, 32, 48, 64, 128, 256] {
        let scaled = downscale_rgba(&rgba, sw, sh, size, size);
        let image = ico::IconImage::from_rgba_data(size, size, scaled);
        let entry = ico::IconDirEntry::encode(&image)
            .map_err(|e| format!("编码 {}px 图标失败：{}", size, e))?;
        icon_dir.add_entry(entry);
    }

    let out_dir = std::env::var("OUT_DIR").map_err(|e| format!("读取 OUT_DIR 失败：{}", e))?;
    let ico_path = std::path::Path::new(&out_dir).join("icon.ico");
    let out_file =
        std::fs::File::create(&ico_path).map_err(|e| format!("创建 icon.ico 失败：{}", e))?;
    icon_dir
        .write(std::io::BufWriter::new(out_file))
        .map_err(|e| format!("写入 icon.ico 失败：{}", e))?;
    Ok(ico_path)
}

// 面积平均下采样：将源 RGBA 图缩放到目标尺寸（适用于任意目标尺寸）
#[cfg(windows)]
fn downscale_rgba(src: &[u8], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for ty in 0..th {
        let y0 = ty * sh / th;
        let y1 = (((ty + 1) * sh / th).max(y0 + 1)).min(sh);
        for tx in 0..tw {
            let x0 = tx * sw / tw;
            let x1 = (((tx + 1) * sw / tw).max(x0 + 1)).min(sw);
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * sw + sx) * 4) as usize;
                    r += src[i] as u64;
                    g += src[i + 1] as u64;
                    b += src[i + 2] as u64;
                    a += src[i + 3] as u64;
                    n += 1;
                }
            }
            let n = n.max(1);
            let o = ((ty * tw + tx) * 4) as usize;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    out
}

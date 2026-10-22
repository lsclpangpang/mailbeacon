//! 附件打开方式：本地化应用名称、32px 图标与系统默认应用排序。

use serde::Serialize;
use std::path::PathBuf;

/// Open the Windows association chooser for one resolved attachment.
#[cfg(windows)]
pub(crate) fn choose_system_app(path: &std::path::Path) -> anyhow::Result<()> {
    use std::{os::windows::ffi::OsStrExt, ptr::null};
    use windows_sys::Win32::{
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        UI::Shell::{OAIF_EXEC, OPENASINFO, SHOpenWithDialog},
    };
    anyhow::ensure!(path.is_file(), "附件文件不存在");
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    anyhow::ensure!(!file[..file.len() - 1].contains(&0), "附件路径无效");
    let initialized = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
    anyhow::ensure!(initialized >= 0, "无法初始化系统打开方式：{initialized:#x}");
    let info = OPENASINFO {
        pcszFile: file.as_ptr(),
        pcszClass: null(),
        oaifInFlags: OAIF_EXEC,
    };
    let result = unsafe { SHOpenWithDialog(std::ptr::null_mut(), &info) };
    unsafe {
        CoUninitialize();
    }
    // Cancelling the chooser is an intentional no-op.
    anyhow::ensure!(
        result >= 0 || result as u32 == 0x800704c7,
        "系统打开方式失败：{result:#x}"
    );
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn choose_system_app(_: &std::path::Path) -> anyhow::Result<()> {
    anyhow::bail!("当前平台请从打开方式菜单选择应用")
}

/// 应用选择器返回的路径必须是真实存在的 `.app` 目录。
pub(crate) fn is_mac_app(path: &std::path::Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// 出现在「打开方式」里的应用。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenWithApp {
    pub name: String,
    pub path: String,
    pub icon: Option<String>,
    pub is_default: bool,
}

pub(crate) fn open_with_app(path: &std::path::Path) -> Option<OpenWithApp> {
    if !is_mac_app(path) {
        return None;
    }
    let fallback = path.file_stem()?.to_string_lossy().to_string();
    if fallback.is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    let (name, icon) = native_metadata(path);
    #[cfg(not(target_os = "macos"))]
    let (name, icon): (Option<String>, Option<String>) = (None, None);
    Some(OpenWithApp {
        name: name.filter(|n| !n.trim().is_empty()).unwrap_or(fallback),
        path: path.display().to_string(),
        icon,
        is_default: false,
    })
}

/// 默认应用始终排第一；按路径去重，保留同名但不同位置的应用。
fn arrange_apps(mut apps: Vec<OpenWithApp>, default: Option<OpenWithApp>) -> Vec<OpenWithApp> {
    let default_path = default.as_ref().map(|a| normalized_path(&a.path));
    if let Some(app) = default {
        apps.push(app);
    }
    for app in &mut apps {
        app.is_default = default_path
            .as_ref()
            .is_some_and(|p| *p == normalized_path(&app.path));
    }
    apps.sort_by(|a, b| {
        b.is_default
            .cmp(&a.is_default)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut seen = std::collections::HashSet::new();
    apps.retain(|app| seen.insert(normalized_path(&app.path)));
    apps
}

fn normalized_path(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// macOS 的 LaunchServices：系统认为能打开这个文件的 App（等同 Finder 的「打开方式」推荐）。
#[cfg(target_os = "macos")]
pub(crate) fn recommended_apps(file: &std::path::Path) -> Vec<OpenWithApp> {
    use std::ffi::{CStr, c_void};
    type CFTypeRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFTypeRef,
            buffer: *const u8,
            buf_len: isize,
            is_directory: u8,
        ) -> CFTypeRef;
        fn CFURLGetFileSystemRepresentation(
            url: CFTypeRef,
            resolve: u8,
            buffer: *mut u8,
            max_len: isize,
        ) -> u8;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        /// kLSRolesAll = 0xFFFFFFFF
        fn LSCopyApplicationURLsForURL(url: CFTypeRef, roles: u32) -> CFTypeRef;
        fn LSCopyDefaultApplicationURLForURL(
            url: CFTypeRef,
            roles: u32,
            error: *mut CFTypeRef,
        ) -> CFTypeRef;
    }

    use std::os::unix::ffi::OsStrExt;
    let bytes = file.as_os_str().as_bytes();
    let mut out = Vec::new();
    let mut default = None;
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        );
        if url.is_null() {
            return out;
        }
        let default_url = LSCopyDefaultApplicationURLForURL(url, 0xFFFF_FFFF, std::ptr::null_mut());
        if !default_url.is_null() {
            let mut buf = vec![0u8; 4096];
            if CFURLGetFileSystemRepresentation(
                default_url,
                1,
                buf.as_mut_ptr(),
                buf.len() as isize,
            ) != 0
            {
                if let Ok(path) = CStr::from_bytes_until_nul(&buf) {
                    let path = PathBuf::from(String::from_utf8_lossy(path.to_bytes()).to_string());
                    default = open_with_app(&path);
                }
            }
            CFRelease(default_url);
        }
        let apps = LSCopyApplicationURLsForURL(url, 0xFFFF_FFFF);
        CFRelease(url);
        if apps.is_null() {
            return arrange_apps(out, default);
        }
        for i in 0..CFArrayGetCount(apps) {
            let item = CFArrayGetValueAtIndex(apps, i);
            if item.is_null() {
                continue;
            }
            let mut buf = vec![0u8; 4096];
            if CFURLGetFileSystemRepresentation(item, 1, buf.as_mut_ptr(), buf.len() as isize) != 0
            {
                if let Ok(path) = CStr::from_bytes_until_nul(&buf) {
                    let path = std::path::PathBuf::from(
                        String::from_utf8_lossy(path.to_bytes()).to_string(),
                    );
                    if is_mac_app(&path) {
                        if let Some(app) = open_with_app(&path) {
                            out.push(app);
                        }
                    }
                }
            }
        }
        CFRelease(apps);
    }
    arrange_apps(out, default)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn recommended_apps(_file: &std::path::Path) -> Vec<OpenWithApp> {
    Vec::new()
}

/// 本机安装的 App（未知类型没有系统推荐时给用户兜底）。
pub(crate) fn installed_apps() -> Vec<OpenWithApp> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
    }
    let mut out = Vec::new();
    for dir in dirs {
        collect_apps(&dir, 0, &mut out);
    }
    arrange_apps(out, None)
}

fn collect_apps(dir: &std::path::Path, depth: usize, out: &mut Vec<OpenWithApp>) {
    if depth > 1 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if is_mac_app(&path) {
            if let Some(app) = open_with_app(&path) {
                out.push(app);
            }
        } else if depth < 1 {
            collect_apps(&path, depth + 1, out);
        }
    }
}

#[cfg(target_os = "macos")]
fn native_metadata(path: &std::path::Path) -> (Option<String>, Option<String>) {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSBundle, NSFileManager, NSString};

    autoreleasepool(|_| {
        let path = NSString::from_str(&path.to_string_lossy());
        let name = NSBundle::bundleWithPath(&path)
            .and_then(|bundle| {
                ["CFBundleDisplayName", "CFBundleName"]
                    .into_iter()
                    .find_map(|key| {
                        let value = bundle.objectForInfoDictionaryKey(&NSString::from_str(key))?;
                        let name = value.downcast_ref::<NSString>()?.to_string();
                        (!name.trim().is_empty()).then_some(name)
                    })
            })
            .or_else(|| {
                let name = NSFileManager::defaultManager()
                    .displayNameAtPath(&path)
                    .to_string();
                let name = name.strip_suffix(".app").unwrap_or(&name).to_string();
                (!name.trim().is_empty()).then_some(name)
            });
        (name, native_icon(&path))
    })
}

/// 在独立的离屏位图里缩略应用图标，不改系统共享图像，也不调用外部转换程序。
#[cfg(target_os = "macos")]
fn native_icon(path: &objc2_foundation::NSString) -> Option<String> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};

    let image = NSWorkspace::sharedWorkspace().iconForFile(path);
    // SAFETY: NULL planes asks AppKit to allocate the 32x32 RGBA storage. The retained
    // bitmap owns that storage throughout drawing/encoding; it is not shared with other threads.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
        NSBitmapImageRep::alloc(), std::ptr::null_mut(), 32, 32, 8, 4, true, false,
        NSDeviceRGBColorSpace, 128, 32,
    )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)?;
    // The current context is thread-local. Always restore it, including unwinding.
    struct RestoreGraphics;
    impl Drop for RestoreGraphics {
        fn drop(&mut self) {
            NSGraphicsContext::restoreGraphicsState_class();
        }
    }
    NSGraphicsContext::saveGraphicsState_class();
    let restore = RestoreGraphics;
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(32., 32.)),
        NSRect::new(NSPoint::new(0., 0.), image.size()),
        NSCompositingOperation::Copy,
        1.,
    );
    context.flushGraphics();
    drop(restore);
    // SAFETY: the empty properties dictionary needs no type-specific values.
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    let bytes = data.to_vec();
    if bytes.is_empty() || bytes.len() > 32 * 1024 {
        return None;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, path: &str) -> OpenWithApp {
        OpenWithApp {
            name: name.into(),
            path: path.into(),
            icon: None,
            is_default: false,
        }
    }

    #[test]
    fn default_is_first_even_when_missing_from_recommendations() {
        let entries = arrange_apps(
            vec![
                app("Alpha", "/Applications/a.app"),
                app("Beta", "/Applications/b.app"),
            ],
            Some(app("Zed", "/Applications/z.app")),
        );
        assert_eq!(
            entries.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            ["Zed", "Alpha", "Beta"]
        );
        assert!(entries[0].is_default);
        assert!(entries[1..].iter().all(|a| !a.is_default));
    }

    #[test]
    fn same_path_is_deduplicated_without_hiding_different_apps_with_same_name() {
        let entries = arrange_apps(
            vec![
                app("Editor", "/Applications/a.app"),
                app("Editor", "/Applications/b.app"),
                app("Alias", "/Applications/a.app"),
            ],
            Some(app("Editor", "/Applications/a.app")),
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/Applications/a.app");
        assert!(entries[0].is_default);
        assert!(!entries[1].is_default);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn application_metadata_falls_back_to_folder_name_without_macos_apis() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("邮件阅读器.app");
        std::fs::create_dir(&path).unwrap();
        let entry = open_with_app(&path).unwrap();
        assert_eq!(entry.name, "邮件阅读器");
        assert_eq!(entry.path, path.display().to_string());
        assert!(entry.icon.is_none());
        assert!(!entry.is_default);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "read-only smoke test against this Mac's installed apps and LaunchServices"]
    fn native_metadata_and_png_default_smoke() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let started = std::time::Instant::now();
        let entry =
            open_with_app(std::path::Path::new("/System/Applications/Preview.app")).unwrap();
        let metadata_elapsed = started.elapsed();
        assert!(!entry.name.is_empty());
        let bytes = STANDARD
            .decode(
                entry
                    .icon
                    .unwrap()
                    .strip_prefix("data:image/png;base64,")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 32);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 32);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("example.png");
        std::fs::write(&file, &bytes).unwrap();
        let started = std::time::Instant::now();
        let choices = recommended_apps(&file);
        let first_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        let repeated = recommended_apps(&file);
        let repeated_elapsed = started.elapsed();
        assert_eq!(
            choices.iter().map(|app| &app.path).collect::<Vec<_>>(),
            repeated.iter().map(|app| &app.path).collect::<Vec<_>>()
        );
        eprintln!(
            "Native app metadata: {:?}; {} recommendations: {:?}; repeat: {:?}",
            metadata_elapsed,
            choices.len(),
            first_elapsed,
            repeated_elapsed
        );
        assert!(!choices.is_empty());
        assert!(choices[0].is_default);
        assert_eq!(choices.iter().filter(|a| a.is_default).count(), 1);
        assert!(
            choices
                .iter()
                .all(|a| is_mac_app(std::path::Path::new(&a.path)))
        );
        // Optional fixture for the interactive preview; contains only public app metadata.
        if let Some(path) = std::env::var_os("MAILBEACON_APP_FIXTURE") {
            std::fs::write(path, serde_json::to_vec_pretty(&choices).unwrap()).unwrap();
        }
    }
}

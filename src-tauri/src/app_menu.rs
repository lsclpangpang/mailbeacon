//! Native menus mirror the main webview's current actions and availability.
use std::{collections::HashMap, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{
    App, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    menu::{CheckMenuItem, MenuBuilder, MenuItem, MenuItemKind, SubmenuBuilder},
};
use tauri_plugin_opener::OpenerExt;

struct Action {
    id: &'static str,
    label: &'static str,
    accelerator: Option<&'static str>,
    check: bool,
}

impl Action {
    const fn new(id: &'static str, label: &'static str) -> Self {
        Self {
            id,
            label,
            accelerator: None,
            check: false,
        }
    }
    const fn key(mut self, accelerator: &'static str) -> Self {
        self.accelerator = Some(accelerator);
        self
    }
    const fn checked(mut self) -> Self {
        self.check = true;
        self
    }
}

const ACTIONS: &[Action] = &[
    Action::new("settings", "设置…").key("CmdOrCtrl+,"),
    Action::new("compose", "新邮件").key("CmdOrCtrl+N"),
    Action::new("check-mail", "收取邮件").key("CmdOrCtrl+Shift+N"),
    Action::new("download-message", "导出当前邮件（.eml）…"),
    Action::new("create-folder", "新建文件夹…"),
    Action::new("search", "搜索邮件").key("CmdOrCtrl+F"),
    Action::new("toggle-sidebar", "显示侧栏").checked(),
    Action::new("toggle-threads", "按会话分组").checked(),
    Action::new("filter-all", "全部邮件").checked(),
    Action::new("filter-unread", "仅未读邮件").checked(),
    Action::new("filter-starred", "仅星标邮件").checked(),
    Action::new("send-records", "发送记录"),
    Action::new("operation-history", "操作记录"),
    Action::new("reply", "回复"),
    Action::new("reply-all", "回复全部"),
    Action::new("forward", "转发"),
    Action::new("archive", "归档"),
    Action::new("move", "移动到…"),
    Action::new("trash", "移到已删除"),
    Action::new("toggle-seen", "标为已读"),
    Action::new("toggle-flagged", "加星标"),
    Action::new("shortcuts", "快捷键说明"),
    Action::new("undo-mail-operation", "撤销邮件操作"),
];

#[derive(Clone, Debug, Deserialize)]
pub struct NativeMenuItemState {
    id: String,
    enabled: bool,
    checked: Option<bool>,
    text: Option<String>,
}

struct NativeMenuState {
    items: HashMap<String, MenuItemKind<tauri::Wry>>,
    latest: Mutex<HashMap<String, NativeMenuItemState>>,
}

fn validate_states(window: &str, states: &[NativeMenuItemState]) -> Result<(), String> {
    if window != "main" {
        return Err("只有主窗口可以更新邮件菜单".into());
    }
    for state in states {
        let action = ACTIONS
            .iter()
            .find(|a| a.id == state.id)
            .ok_or_else(|| "未知菜单操作".to_string())?;
        if state.checked.is_some() && !action.check {
            return Err("此菜单不支持勾选状态".into());
        }
        if let Some(text) = &state.text {
            if !matches!(
                action.id,
                "toggle-seen" | "toggle-flagged" | "trash" | "check-mail"
            ) || text.trim().is_empty()
                || text.chars().count() > 80
                || text.chars().any(char::is_control)
            {
                return Err("此菜单标题无效".into());
            }
        }
    }
    Ok(())
}

fn can_dispatch(window: &str, focused: bool, enabled: bool) -> bool {
    window == "main" && focused && enabled
}

fn main_is_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_focused().unwrap_or(false))
        && !app
            .get_webview_window("about")
            .is_some_and(|w| w.is_focused().unwrap_or(false))
}

// Native menu setters run on the UI thread. Never hold the state lock while
// waiting for that thread; focus events also read this state.
fn apply_latest(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<NativeMenuState>();
    let latest = state.latest.lock().unwrap().clone();
    let focused = main_is_focused(app);
    for (id, item) in &state.items {
        let current = latest.get(id);
        let enabled = current.is_some_and(|s| s.enabled) && focused;
        match item {
            MenuItemKind::MenuItem(item) => {
                item.set_enabled(enabled)?;
                if let Some(text) = current.and_then(|s| s.text.as_ref()) {
                    item.set_text(text)?;
                }
            }
            MenuItemKind::Check(item) => {
                item.set_enabled(enabled)?;
                item.set_checked(current.and_then(|s| s.checked).unwrap_or(false))?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn refresh_for_focus(app: &AppHandle) {
    // Focusing/closing About may race with a webview state update. Evaluate
    // focus inside the queued UI callback so an old update cannot re-enable it.
    if app.try_state::<NativeMenuState>().is_none() {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(error) = apply_latest(&handle) {
            eprintln!("更新菜单状态失败：{error}");
        }
    });
}

#[tauri::command]
pub fn set_native_menu_state(
    app: AppHandle,
    window: WebviewWindow,
    states: Vec<NativeMenuItemState>,
) -> Result<(), String> {
    validate_states(window.label(), &states)?;
    {
        let state = app.state::<NativeMenuState>();
        let mut latest = state.latest.lock().map_err(|_| "菜单状态不可用")?;
        for value in states {
            latest.insert(value.id.clone(), value);
        }
    }
    refresh_for_focus(&app);
    Ok(())
}

fn validate_popup_caller(window: &str, windows: bool) -> Result<(), String> {
    if window != "main" {
        return Err("只有主窗口可以打开应用菜单".into());
    }
    if !windows {
        return Err("此平台请使用系统菜单栏".into());
    }
    Ok(())
}

fn validate_popup_position(
    x: f64,
    y: f64,
    size: tauri::PhysicalSize<u32>,
    scale: f64,
) -> Result<tauri::LogicalPosition<f64>, String> {
    if !scale.is_finite() || scale <= 0.0 || size.width == 0 || size.height == 0 {
        return Err("主窗口尺寸不可用".into());
    }
    let width = f64::from(size.width) / scale;
    let height = f64::from(size.height) / scale;
    if !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || x < 0.0
        || y < 0.0
        || x > width
        || y > height
    {
        return Err("应用菜单位置超出主窗口".into());
    }
    Ok(tauri::LogicalPosition::new(x, y))
}

#[tauri::command]
pub async fn popup_native_menu(
    app: AppHandle,
    window: WebviewWindow,
    x: f64,
    y: f64,
) -> Result<(), String> {
    validate_popup_caller(window.label(), cfg!(target_os = "windows"))?;
    if !main_is_focused(&app) {
        return Err("请先激活主窗口".into());
    }
    let position = validate_popup_position(
        x,
        y,
        window.inner_size().map_err(|e| e.to_string())?,
        window.scale_factor().map_err(|e| e.to_string())?,
    )?;
    let menu = app.menu().ok_or("应用菜单不可用")?;
    apply_latest(&app).map_err(|e| e.to_string())?;
    // Like Tauri's menu plugin, enter from an async command so the WebView IPC
    // callback returns before the native popup starts its nested event loop.
    // Reuse the registered tree and dispatch without holding a state lock.
    // The logical anchor is client-relative; Tauri marshals to the UI thread.
    window
        .popup_menu_at(&menu, position)
        .map_err(|e| e.to_string())
}

fn project_link(kind: &str) -> Option<&'static str> {
    match kind {
        "project" => Some("https://github.com/lsclpangpang/mailbeacon"),
        "readme" => Some("https://github.com/lsclpangpang/mailbeacon/blob/main/README.md"),
        "issues" => Some("https://github.com/lsclpangpang/mailbeacon/issues"),
        "releases" => Some("https://github.com/lsclpangpang/mailbeacon/releases"),
        _ => None,
    }
}

fn open_fixed_link(app: &AppHandle, kind: &str) -> Result<(), String> {
    let url = project_link(kind).ok_or("未知项目链接")?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("{e}"))
}

#[tauri::command]
pub fn open_project_link(
    app: AppHandle,
    window: WebviewWindow,
    kind: String,
) -> Result<(), String> {
    if !matches!(window.label(), "main" | "about") {
        return Err("此窗口不能打开项目链接".into());
    }
    open_fixed_link(&app, &kind)
}

#[derive(Serialize)]
pub struct AboutInfo {
    version: String,
}

#[tauri::command]
pub fn about_info(app: AppHandle, window: WebviewWindow) -> Result<AboutInfo, String> {
    if window.label() != "about" {
        return Err("请从关于窗口读取版本信息".into());
    }
    Ok(AboutInfo {
        version: app.package_info().version.to_string(),
    })
}

fn allow_about_navigation(url: &tauri::Url) -> bool {
    let local = matches!(
        (url.scheme(), url.host_str()),
        ("tauri", Some("localhost")) | ("http" | "https", Some("tauri.localhost"))
    );
    local && url.path() == "/about.html" && url.query().is_none()
}

fn show_about(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("about") {
        #[cfg(target_os = "windows")]
        window.hide_menu()?;
        window.unminimize()?;
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }
    let builder = WebviewWindowBuilder::new(app, "about", WebviewUrl::App("about.html".into()))
        .title("关于 MailBeacon")
        .inner_size(400.0, 340.0)
        .resizable(false)
        .maximizable(false)
        .center()
        .on_navigation(allow_about_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    #[cfg(target_os = "windows")]
    {
        // New windows inherit the registered app menu. Hide its bar before the
        // first Windows frame is shown, while retaining native accelerators.
        let window = builder.visible(false).build()?;
        window.hide_menu()?;
        window.show()?;
        window.set_focus()?;
    }
    #[cfg(not(target_os = "windows"))]
    builder.build()?;
    Ok(())
}

fn report_error(app: &AppHandle, error: impl std::fmt::Display) {
    eprintln!("菜单操作失败：{error}");
    let message = format!("菜单操作失败：{error}");
    let _ = app.emit_to("main", "native-menu-error", &message);
    let _ = app.emit_to("about", "native-menu-error", &message);
}

fn dispatch(app: &AppHandle, id: &str) {
    match id {
        "app-about" => {
            if let Err(error) = show_about(app) {
                report_error(app, error);
            }
        }
        "app-show-main" => super::show_main_window(app),
        "help-readme" | "help-project" | "help-issues" | "help-releases" => {
            if let Err(error) = open_fixed_link(app, id.trim_start_matches("help-")) {
                report_error(app, error);
            }
        }
        _ => {
            let Some(action) = id.strip_prefix("native-") else {
                return;
            };
            let enabled = app
                .state::<NativeMenuState>()
                .latest
                .lock()
                .unwrap()
                .get(action)
                .is_some_and(|state| state.enabled);
            if can_dispatch("main", main_is_focused(app), enabled) {
                if let Err(error) = app.emit_to("main", "native-menu", action) {
                    report_error(app, error);
                }
            }
            // Cocoa toggles checkmarks before dispatch; frontend state is the
            // authority, including when the action is rejected or fails.
            refresh_for_focus(app);
        }
    }
}

pub fn install(app: &App) -> tauri::Result<()> {
    let mut items = HashMap::new();
    for action in ACTIONS {
        let id = format!("native-{}", action.id);
        let item = if action.check {
            MenuItemKind::Check(CheckMenuItem::with_id(
                app,
                id,
                action.label,
                false,
                false,
                action.accelerator,
            )?)
        } else {
            MenuItemKind::MenuItem(MenuItem::with_id(
                app,
                id,
                action.label,
                false,
                action.accelerator,
            )?)
        };
        items.insert(action.id.to_string(), item);
    }
    let item = |id: &str| -> &dyn tauri::menu::IsMenuItem<tauri::Wry> {
        match items.get(id).expect("declared menu action") {
            MenuItemKind::MenuItem(item) => item,
            MenuItemKind::Check(item) => item,
            _ => unreachable!(),
        }
    };
    let application = SubmenuBuilder::new(app, "MailBeacon")
        .text("app-about", "关于 MailBeacon")
        .separator()
        .item(item("settings"));
    #[cfg(target_os = "macos")]
    let application = application
        .separator()
        .hide_with_text("隐藏 MailBeacon")
        .hide_others_with_text("隐藏其他窗口")
        .show_all_with_text("显示全部");
    let application = application
        .separator()
        .quit_with_text("退出 MailBeacon")
        .build()?;
    let file = SubmenuBuilder::new(app, "文件")
        .item(item("compose"))
        .item(item("check-mail"))
        .separator()
        .item(item("download-message"))
        .item(item("create-folder"))
        .separator()
        .close_window_with_text("关闭窗口")
        .build()?;
    let edit = SubmenuBuilder::new(app, "编辑")
        .undo_with_text("撤销")
        .redo_with_text("重做")
        .separator()
        .cut_with_text("剪切")
        .copy_with_text("拷贝")
        .paste_with_text("粘贴")
        .select_all_with_text("全选")
        .separator()
        .item(item("search"))
        .item(item("undo-mail-operation"))
        .build()?;
    let view = SubmenuBuilder::new(app, "视图")
        .item(item("toggle-sidebar"))
        .item(item("toggle-threads"))
        .separator()
        .item(item("filter-all"))
        .item(item("filter-unread"))
        .item(item("filter-starred"))
        .separator()
        .item(item("send-records"))
        .item(item("operation-history"));
    #[cfg(target_os = "macos")]
    let view = view.separator().fullscreen_with_text("进入全屏");
    let view = view.build()?;
    let mail = SubmenuBuilder::new(app, "邮件")
        .item(item("reply"))
        .item(item("reply-all"))
        .item(item("forward"))
        .separator()
        .item(item("archive"))
        .item(item("move"))
        .item(item("trash"))
        .separator()
        .item(item("toggle-seen"))
        .item(item("toggle-flagged"))
        .build()?;
    let window = SubmenuBuilder::new(app, "窗口")
        .text("app-show-main", "显示主窗口")
        .separator()
        .minimize_with_text("最小化")
        .close_window_with_text("关闭窗口")
        .build()?;
    let help = SubmenuBuilder::new(app, "帮助")
        .text("help-readme", "使用说明")
        .item(item("shortcuts"))
        .separator()
        .text("help-project", "GitHub 项目")
        .text("help-issues", "反馈问题")
        .text("help-releases", "查看版本更新")
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&application, &file, &edit, &view, &mail, &window, &help])
        .build()?;
    app.manage(NativeMenuState {
        items,
        latest: Mutex::new(HashMap::new()),
    });
    app.set_menu(menu)?;
    #[cfg(target_os = "windows")]
    {
        // Keep the menu registered: hide preserves the accelerator table and
        // window command handler, whereas remove_menu would unregister them.
        app.hide_menu()?;
    }
    #[cfg(target_os = "macos")]
    window.set_as_windows_menu_for_nsapp()?;
    app.on_menu_event(|app, event| dispatch(app, event.id().as_ref()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_command_is_async_so_native_menu_does_not_reenter_the_ipc_callback() {
        fn assert_async<F: std::future::Future<Output = Result<(), String>>>(
            _: fn(AppHandle, WebviewWindow, f64, f64) -> F,
        ) {
        }
        assert_async(popup_native_menu);
    }

    #[test]
    fn popup_menu_is_only_available_to_the_main_windows_webview() {
        assert!(validate_popup_caller("main", true).is_ok());
        assert!(validate_popup_caller("about", true).is_err());
        assert!(validate_popup_caller("compose", true).is_err());
        assert!(validate_popup_caller("main", false).is_err());
    }

    #[test]
    fn popup_anchor_uses_client_logical_coordinates_at_high_dpi() {
        let size = tauri::PhysicalSize::new(1600, 1200);
        let position = validate_popup_position(24.0, 42.5, size, 2.0).unwrap();
        assert_eq!((position.x, position.y), (24.0, 42.5));
        assert!(validate_popup_position(800.0, 600.0, size, 2.0).is_ok());
        assert!(validate_popup_position(800.1, 42.5, size, 2.0).is_err());
        assert!(validate_popup_position(24.0, 600.1, size, 2.0).is_err());
    }

    #[test]
    fn popup_anchor_rejects_invalid_numbers_and_outside_client_bounds() {
        let size = tauri::PhysicalSize::new(1000, 700);
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            assert!(validate_popup_position(value, 10.0, size, 1.0).is_err());
            assert!(validate_popup_position(10.0, value, size, 1.0).is_err());
        }
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(validate_popup_position(10.0, 10.0, size, scale).is_err());
        }
        assert!(validate_popup_position(0.0, 0.0, size, 1.0).is_ok());
        assert!(validate_popup_position(0.0, 0.0, tauri::PhysicalSize::new(0, 700), 1.0).is_err());
        assert!(validate_popup_position(0.0, 0.0, tauri::PhysicalSize::new(1000, 0), 1.0).is_err());
    }

    #[test]
    fn menu_states_reject_unknown_actions_and_non_main_callers() {
        let state = NativeMenuItemState {
            id: "trash".into(),
            enabled: true,
            checked: None,
            text: None,
        };
        assert!(validate_states("main", &[state.clone()]).is_ok());
        assert!(validate_states("about", &[state]).is_err());
        assert!(
            validate_states(
                "main",
                &[NativeMenuItemState {
                    id: "quit".into(),
                    enabled: true,
                    checked: None,
                    text: None,
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn dynamic_menu_properties_are_limited_to_declared_items() {
        let mut state = NativeMenuItemState {
            id: "trash".into(),
            enabled: true,
            checked: None,
            text: Some("已在已删除中".into()),
        };
        assert!(validate_states("main", &[state.clone()]).is_ok());
        state.id = "compose".into();
        assert!(validate_states("main", &[state.clone()]).is_err());
        state.text = None;
        state.checked = Some(true);
        assert!(validate_states("main", &[state.clone()]).is_err());
        state.id = "filter-unread".into();
        assert!(validate_states("main", &[state]).is_ok());
    }

    #[test]
    fn menu_actions_are_unique_and_single_letter_shortcuts_are_never_native() {
        let mut names = std::collections::HashSet::new();
        for action in ACTIONS {
            assert!(names.insert(action.id));
            if let Some(accelerator) = action.accelerator {
                assert!(accelerator.starts_with("CmdOrCtrl+"));
            }
        }
        assert_eq!(names.len(), 23);
    }

    #[test]
    fn business_actions_require_main_focus_and_frontend_permission() {
        assert!(can_dispatch("main", true, true));
        assert!(!can_dispatch("about", true, true));
        assert!(!can_dispatch("main", false, true));
        assert!(!can_dispatch("main", true, false));
    }

    #[test]
    fn project_links_are_fixed_and_never_arbitrary_urls() {
        assert_eq!(
            project_link("project"),
            Some("https://github.com/lsclpangpang/mailbeacon")
        );
        assert_eq!(
            project_link("issues"),
            Some("https://github.com/lsclpangpang/mailbeacon/issues")
        );
        assert_eq!(
            project_link("releases"),
            Some("https://github.com/lsclpangpang/mailbeacon/releases")
        );
        assert_eq!(project_link("https://example.com"), None);
        assert_eq!(project_link("file:///etc/passwd"), None);
    }

    #[test]
    fn about_never_navigates_to_remote_content_or_another_local_page() {
        for url in [
            "tauri://localhost/about.html",
            "http://tauri.localhost/about.html",
            "https://tauri.localhost/about.html",
        ] {
            assert!(allow_about_navigation(&url.parse().unwrap()), "{url}");
        }
        for url in [
            "https://github.com/lsclpangpang/mailbeacon",
            "http://localhost/about.html",
            "tauri://localhost/index.html",
            "file:///about.html",
        ] {
            assert!(!allow_about_navigation(&url.parse().unwrap()), "{url}");
        }
    }
}

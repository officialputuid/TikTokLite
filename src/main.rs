#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(any(windows, test))]
const TIKTOK_URL: &str = "https://www.tiktok.com/";
#[cfg(any(windows, test))]
const MENU_IDS: [&str; 14] = [
    "open",
    "pause_media",
    "settings_close_tray",
    "settings_autostart",
    "settings_start_minimized",
    "settings_pause_hidden",
    "permission_notifications",
    "permission_camera",
    "permission_microphone",
    "reload",
    "clear_site_data",
    "check_update",
    "github",
    "quit",
];

#[cfg(not(windows))]
fn main() {
    println!("TikTok Lite targets Windows (WebView2).");
}

#[cfg(windows)]
mod windows_app {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        },
        time::Duration,
    };
    use tauri::{
        menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
        webview::{NewWindowResponse, WebviewWindowBuilder},
        AppHandle, Manager, WebviewUrl, WindowEvent,
    };
    use tauri_plugin_autostart::ManagerExt;
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    use tiktok_lite::{
        classify_navigation, classify_popup_navigation, lifecycle_action, load_settings,
        media_control_script, permission_origin_allowed, release_version_is_newer,
        restored_from_minimized, save_settings, update_error_message, CloseBehavior,
        LifecycleAction, NavigationAction, PopupNavigationAction, Settings, WindowState,
    };
    use webview2_com::{
        take_pwstr,
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2, ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND,
            COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            COREWEBVIEW2_PERMISSION_STATE, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            COREWEBVIEW2_PERMISSION_STATE_DENY,
        },
        PermissionRequestedEventHandler, SetPermissionStateCompletedHandler,
    };
    use windows_core::{Interface, HSTRING, PCWSTR};

    use super::{MENU_IDS, TIKTOK_URL};

    const MAIN: &str = "main";
    const OPEN: &str = MENU_IDS[0];
    const PAUSE_MEDIA: &str = MENU_IDS[1];
    const CLOSE_TRAY: &str = MENU_IDS[2];
    const AUTOSTART: &str = MENU_IDS[3];
    const START_MINIMIZED: &str = MENU_IDS[4];
    const PAUSE_HIDDEN: &str = MENU_IDS[5];
    const NOTIFICATIONS: &str = MENU_IDS[6];
    const CAMERA: &str = MENU_IDS[7];
    const MICROPHONE: &str = MENU_IDS[8];
    const RELOAD: &str = MENU_IDS[9];
    const CLEAR_SITE_DATA: &str = MENU_IDS[10];
    const CHECK_UPDATE: &str = MENU_IDS[11];
    const GITHUB: &str = MENU_IDS[12];
    const QUIT: &str = MENU_IDS[13];
    const GITHUB_URL: &str = "https://github.com/officialputuid/TikTokLite";
    const RELEASE_API: &str =
        "https://api.github.com/repos/officialputuid/TikTokLite/releases/latest";
    static UPDATE_CHECK_RUNNING: AtomicBool = AtomicBool::new(false);

    #[derive(serde::Deserialize)]
    struct ReleaseInfo {
        tag_name: String,
        html_url: String,
    }

    struct State {
        settings: Mutex<Settings>,
        settings_path: std::path::PathBuf,
        media_paused: AtomicBool,
        was_minimized: AtomicBool,
        pause_media_item: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    }

    fn settings(app: &AppHandle) -> Settings {
        app.state::<State>()
            .settings
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    fn update_settings(app: &AppHandle, change: impl FnOnce(&mut Settings)) -> std::io::Result<()> {
        let state = app.state::<State>();
        let mut settings = state
            .settings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut changed = settings.clone();
        change(&mut changed);
        save_settings(&state.settings_path, &changed)?;
        *settings = changed;
        Ok(())
    }

    fn show_error(app: &AppHandle, message: &str) {
        app.dialog()
            .message(message)
            .title("TikTok Lite")
            .kind(MessageDialogKind::Error)
            .show(|_| {});
    }

    unsafe fn set_profile_permission(
        core: &ICoreWebView2,
        kind: COREWEBVIEW2_PERMISSION_KIND,
        allowed: bool,
        reload_app: Option<AppHandle>,
    ) -> windows_core::Result<()> {
        let core13: ICoreWebView2_13 = core.cast()?;
        let profile: ICoreWebView2Profile4 = core13.Profile()?.cast()?;
        let origin = HSTRING::from("https://www.tiktok.com");
        let state: COREWEBVIEW2_PERMISSION_STATE = if allowed {
            COREWEBVIEW2_PERMISSION_STATE_ALLOW
        } else {
            COREWEBVIEW2_PERMISSION_STATE_DENY
        };
        let handler = SetPermissionStateCompletedHandler::create(Box::new(move |result| {
            match result {
                Ok(()) => {
                    if let Some(app) = reload_app.as_ref() {
                        if let Some(window) = app.get_webview_window(MAIN) {
                            if let Err(error) = window.reload() {
                                eprintln!("failed to reload after permission update: {error}");
                            }
                        }
                    }
                }
                Err(error) => eprintln!("failed to persist WebView2 permission: {error}"),
            }
            Ok(())
        }));
        profile.SetPermissionState(kind, PCWSTR(origin.as_ptr()), state, &handler)
    }

    fn sync_permissions(app: &AppHandle) {
        let current = settings(app);
        if let Some(window) = app.get_webview_window(MAIN) {
            let result = window.with_webview(move |webview| unsafe {
                let Ok(core) = webview.controller().CoreWebView2() else {
                    eprintln!("failed to access WebView2 permissions");
                    return;
                };
                for (kind, allowed) in [
                    (
                        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                        current.allow_notifications,
                    ),
                    (COREWEBVIEW2_PERMISSION_KIND_CAMERA, current.allow_camera),
                    (
                        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                        current.allow_microphone,
                    ),
                    (COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION, false),
                ] {
                    if let Err(error) = set_profile_permission(&core, kind, allowed, None) {
                        eprintln!("failed to set WebView2 permission: {error}");
                    }
                }
            });
            if let Err(error) = result {
                eprintln!("failed to sync WebView2 permissions: {error}");
            }
        }
    }

    fn apply_permission(
        app: &AppHandle,
        item: &CheckMenuItem<tauri::Wry>,
        old: bool,
        kind: COREWEBVIEW2_PERMISSION_KIND,
        change: impl FnOnce(&mut Settings),
    ) {
        if !persist_toggle(app, item, old, change) {
            return;
        }
        let Some(window) = app.get_webview_window(MAIN) else {
            show_error(
                app,
                "Permission was saved, but the TikTok window is unavailable.",
            );
            return;
        };
        let reload_app = app.clone();
        let result = window.with_webview(move |webview| unsafe {
            match webview.controller().CoreWebView2() {
                Ok(core) => {
                    if let Err(error) = set_profile_permission(&core, kind, !old, Some(reload_app))
                    {
                        eprintln!("failed to set WebView2 permission: {error}");
                    }
                }
                Err(error) => eprintln!("failed to access WebView2 permissions: {error}"),
            }
        });
        if let Err(error) = result {
            eprintln!("failed to apply WebView2 permission: {error}");
            show_error(
                app,
                "Permission was saved but could not be applied until restart.",
            );
        }
    }

    fn check_for_updates(app: &AppHandle) {
        if UPDATE_CHECK_RUNNING.swap(true, Ordering::AcqRel) {
            return;
        }
        let app = app.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<ReleaseInfo, Option<u16>> {
                let mut response = ureq::get(RELEASE_API)
                    .header("Accept", "application/vnd.github+json")
                    .header("User-Agent", "TikTok-Lite")
                    .config()
                    .timeout_global(Some(Duration::from_secs(10)))
                    .build()
                    .call()
                    .map_err(|error| match error {
                        ureq::Error::StatusCode(status) => Some(status),
                        _ => None,
                    })?;
                response
                    .body_mut()
                    .read_json::<ReleaseInfo>()
                    .map_err(|_| None)
            })();
            UPDATE_CHECK_RUNNING.store(false, Ordering::Release);

            match result {
                Ok(release)
                    if release_version_is_newer(&release.tag_name, env!("CARGO_PKG_VERSION")) =>
                {
                    let download_url = release.html_url;
                    app.dialog()
                        .message(format!(
                            "Version {} is available.\nInstalled: {}",
                            release.tag_name,
                            env!("CARGO_PKG_VERSION")
                        ))
                        .title("TikTok Lite Update")
                        .buttons(MessageDialogButtons::OkCancelCustom(
                            "Open Release".into(),
                            "Later".into(),
                        ))
                        .show(move |open| {
                            if open {
                                let _ = tauri_plugin_opener::open_url(download_url, None::<&str>);
                            }
                        });
                }
                Ok(_) => {
                    app.dialog()
                        .message(format!(
                            "TikTok Lite {} is up to date.",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .title("TikTok Lite Update")
                        .show(|_| {});
                }
                Err(status) => {
                    app.dialog()
                        .message(update_error_message(status))
                        .title("TikTok Lite Update")
                        .kind(MessageDialogKind::Error)
                        .show(|_| {});
                }
            }
        });
    }

    fn persist_toggle(
        app: &AppHandle,
        item: &CheckMenuItem<tauri::Wry>,
        old_checked: bool,
        change: impl FnOnce(&mut Settings),
    ) -> bool {
        if let Err(error) = update_settings(app, change) {
            eprintln!("failed to save settings: {error}");
            if let Err(restore_error) = item.set_checked(old_checked) {
                eprintln!("failed to restore menu check state: {restore_error}");
            }
            show_error(app, "Failed to save settings. Your change was not applied.");
            false
        } else {
            true
        }
    }

    fn set_media_paused(app: &AppHandle, paused: bool) {
        app.state::<State>()
            .media_paused
            .store(paused, Ordering::Release);
        if let Ok(item) = app.state::<State>().pause_media_item.lock() {
            if let Some(item) = item.as_ref() {
                if let Err(error) = item.set_checked(paused) {
                    eprintln!("failed to synchronize Pause Media check state: {error}");
                }
            }
        }
        if let Some(window) = app.get_webview_window(MAIN) {
            if let Err(error) = window.eval(media_control_script(paused)) {
                eprintln!("failed to update media state: {error}");
            }
        }
    }

    fn show_main(app: &AppHandle) {
        if let Some(window) = app.get_webview_window(MAIN) {
            if let Err(error) = window.unminimize() {
                eprintln!("failed to unminimize main window: {error}");
            }
            if let Err(error) = window.show() {
                eprintln!("failed to show main window: {error}");
            }
            if let Err(error) = window.set_focus() {
                eprintln!("failed to focus main window: {error}");
            }
            set_media_paused(app, false);
        }
    }

    fn hide_main(app: &AppHandle) {
        let current = settings(app);
        match lifecycle_action(WindowState::Hidden, &current) {
            LifecycleAction::HideAndPause => set_media_paused(app, true),
            LifecycleAction::Hide | LifecycleAction::Exit | LifecycleAction::Show => {}
        }
        if let Some(window) = app.get_webview_window(MAIN) {
            if let Err(error) = window.hide() {
                eprintln!("failed to hide main window: {error}");
            }
        }
    }

    fn open_external(url: &str) {
        if let Err(error) = tauri_plugin_opener::open_url(url, None::<&str>) {
            eprintln!("failed to open external URL: {error}");
        }
    }

    fn clear_site_data(app: &AppHandle) {
        let app_for_dialog = app.clone();
        app.dialog()
            .message("Clear all TikTok site data? This signs you out.")
            .title("Clear TikTok Site Data")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Clear Data".into(),
                "Cancel".into(),
            ))
            .show(move |confirmed| {
                if !confirmed {
                    return;
                }
                let Some(window) = app_for_dialog.get_webview_window(MAIN) else {
                    eprintln!("failed to clear site data: main window is unavailable");
                    show_error(
                        &app_for_dialog,
                        "Failed to clear TikTok site data: main window is unavailable.",
                    );
                    return;
                };
                let callback_app = app_for_dialog.clone();
                let result = window.with_webview(move |webview| unsafe {
                    use webview2_com::{
                        ClearBrowsingDataCompletedHandler,
                        Microsoft::Web::WebView2::Win32::{
                            ICoreWebView2Profile2, ICoreWebView2_13,
                        },
                    };
                    use windows_core::Interface;

                    let clear_result = (|| -> windows_core::Result<()> {
                        let core: ICoreWebView2_13 = webview.controller().CoreWebView2()?.cast()?;
                        let profile: ICoreWebView2Profile2 = core.Profile()?.cast()?;
                        let completion_app = callback_app.clone();
                        let handler = ClearBrowsingDataCompletedHandler::create(Box::new(
                            move |clear_result| {
                                if let Some(window) = completion_app.get_webview_window(MAIN) {
                                    if clear_result.is_ok() {
                                        if let Err(error) =
                                            window.navigate(TIKTOK_URL.parse().unwrap())
                                        {
                                            eprintln!("failed to reload after site-data cleanup: {error}");
                                        }
                                        completion_app
                                            .dialog()
                                            .message("TikTok site data cleared.")
                                            .title("TikTok Lite")
                                            .show(|_| {});
                                    } else {
                                        show_error(
                                            &completion_app,
                                            "Failed to clear TikTok site data.",
                                        );
                                    }
                                } else {
                                    eprintln!("site-data cleanup completed after main window closed");
                                    show_error(
                                        &completion_app,
                                        "Site data cleanup finished, but main window is unavailable.",
                                    );
                                }
                                Ok(())
                            },
                        ));
                        profile.ClearBrowsingDataAll(&handler)
                    })();
                    if let Err(error) = clear_result {
                        eprintln!("failed to access WebView2 profile: {error}");
                        show_error(&callback_app, "Failed to clear TikTok site data.");
                    }
                });
                if let Err(error) = result {
                    eprintln!("failed to start site-data cleanup: {error}");
                    app_for_dialog
                        .dialog()
                        .message("Failed to clear TikTok site data.")
                        .title("TikTok Lite")
                        .kind(MessageDialogKind::Error)
                        .show(|_| {});
                }
            });
    }

    pub fn run() {
        tauri::Builder::default()
            .plugin(tauri_plugin_single_instance::init(|app, _, _| {
                show_main(app);
            }))
            .plugin(tauri_plugin_autostart::Builder::new().build())
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_opener::init())
            .setup(|app| {
                let settings_path = app.path().app_config_dir()?.join("settings.json");
                if let Some(parent) = settings_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let initial = load_settings(&settings_path);
                app.manage(State {
                    settings: Mutex::new(initial.clone()),
                    settings_path,
                    media_paused: AtomicBool::new(initial.start_minimized),
                    was_minimized: AtomicBool::new(initial.start_minimized),
                    pause_media_item: Mutex::new(None),
                });

                let autostart_enabled = match app.autolaunch().is_enabled() {
                    Ok(enabled) => {
                        if initial.autostart != enabled {
                            if let Err(error) = update_settings(app.handle(), |value| {
                                value.autostart = enabled;
                            }) {
                                eprintln!("failed to reconcile autostart setting: {error}");
                                show_error(app.handle(), "Failed to save Windows startup status.");
                            }
                        }
                        enabled
                    }
                    Err(error) => {
                        eprintln!("failed to read autostart status: {error}");
                        show_error(
                            app.handle(),
                            "Failed to read Windows startup status. Saved setting was preserved.",
                        );
                        initial.autostart
                    }
                };

                let pause_media = CheckMenuItemBuilder::with_id(PAUSE_MEDIA, "Pause Media")
                    .checked(initial.start_minimized)
                    .build(app)?;
                *app.state::<State>()
                    .pause_media_item
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(pause_media.clone());
                let close_tray = CheckMenuItemBuilder::with_id(CLOSE_TRAY, "Hide to tray on close")
                    .checked(initial.close_behavior == CloseBehavior::Tray)
                    .build(app)?;
                let autostart = CheckMenuItemBuilder::with_id(AUTOSTART, "Start with Windows")
                    .checked(autostart_enabled)
                    .build(app)?;
                let start_minimized =
                    CheckMenuItemBuilder::with_id(START_MINIMIZED, "Start minimized")
                        .checked(initial.start_minimized)
                        .build(app)?;
                let pause_hidden =
                    CheckMenuItemBuilder::with_id(PAUSE_HIDDEN, "Pause media when hidden")
                        .checked(initial.pause_media_when_hidden)
                        .build(app)?;
                let notifications =
                    CheckMenuItemBuilder::with_id(NOTIFICATIONS, "Notifications")
                        .checked(initial.allow_notifications)
                        .build(app)?;
                let camera = CheckMenuItemBuilder::with_id(CAMERA, "Camera")
                    .checked(initial.allow_camera)
                    .build(app)?;
                let microphone = CheckMenuItemBuilder::with_id(MICROPHONE, "Microphone")
                    .checked(initial.allow_microphone)
                    .build(app)?;
                let location = MenuItemBuilder::new("Location (Blocked)")
                    .enabled(false)
                    .build(app)?;
                let permissions_menu = SubmenuBuilder::new(app, "Permissions")
                    .item(&notifications)
                    .item(&camera)
                    .item(&microphone)
                    .item(&location)
                    .build()?;
                let settings_menu = SubmenuBuilder::new(app, "Settings")
                    .item(&close_tray)
                    .item(&autostart)
                    .item(&start_minimized)
                    .item(&pause_hidden)
                    .item(&permissions_menu)
                    .build()?;
                let app_name = MenuItemBuilder::new("TikTok Lite")
                    .enabled(false)
                    .build(app)?;
                let version =
                    MenuItemBuilder::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                        .enabled(false)
                        .build(app)?;
                let developer = MenuItemBuilder::new("Developer: officialputuid")
                    .enabled(false)
                    .build(app)?;
                let about = SubmenuBuilder::new(app, "About")
                    .item(&app_name)
                    .item(&version)
                    .item(&developer)
                    .separator()
                    .text(CHECK_UPDATE, "Check for Updates")
                    .text(GITHUB, "GitHub")
                    .build()?;
                let menu = MenuBuilder::new(app)
                    .text(OPEN, "Open TikTok Lite")
                    .item(&pause_media)
                    .item(&settings_menu)
                    .separator()
                    .text(RELOAD, "Reload")
                    .text(CLEAR_SITE_DATA, "Clear Site Data")
                    .item(&about)
                    .separator()
                    .text(QUIT, "Quit")
                    .build()?;

                TrayIconBuilder::new()
                    .icon(
                        app.default_window_icon()
                            .cloned()
                            .expect("app icon missing"),
                    )
                    .tooltip("TikTok Lite")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event({
                        let close_tray = close_tray.clone();
                        let autostart = autostart.clone();
                        let start_minimized = start_minimized.clone();
                        let pause_hidden = pause_hidden.clone();
                        let notifications = notifications.clone();
                        let camera = camera.clone();
                        let microphone = microphone.clone();
                        move |app, event| match event.id().as_ref() {
                            OPEN => show_main(app),
                            PAUSE_MEDIA => {
                                let paused =
                                    !app.state::<State>().media_paused.load(Ordering::Acquire);
                                set_media_paused(app, paused);
                            }
                            CLOSE_TRAY => {
                                let old = settings(app).close_behavior == CloseBehavior::Tray;
                                persist_toggle(app, &close_tray, old, |value| {
                                    value.close_behavior = if old {
                                        CloseBehavior::Exit
                                    } else {
                                        CloseBehavior::Tray
                                    };
                                });
                            }
                            AUTOSTART => {
                                let old = settings(app).autostart;
                                let plugin_result = if old {
                                    app.autolaunch().disable()
                                } else {
                                    app.autolaunch().enable()
                                };
                                match plugin_result {
                                    Ok(()) => {
                                        if !persist_toggle(app, &autostart, old, |value| {
                                            value.autostart = !old;
                                        }) {
                                            let rollback = if old {
                                                app.autolaunch().enable()
                                            } else {
                                                app.autolaunch().disable()
                                            };
                                            if let Err(error) = rollback {
                                                eprintln!("failed to roll back autostart: {error}");
                                                show_error(
                                                    app,
                                                    "Failed to roll back Windows startup setting.",
                                                );
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        eprintln!("failed to update autostart: {error}");
                                        if let Err(restore_error) = autostart.set_checked(old) {
                                            eprintln!(
                                                "failed to restore autostart check state: {restore_error}"
                                            );
                                        }
                                        show_error(
                                            app,
                                            "Failed to update Windows startup setting.",
                                        );
                                    }
                                }
                            }
                            START_MINIMIZED => {
                                let old = settings(app).start_minimized;
                                persist_toggle(app, &start_minimized, old, |value| {
                                    value.start_minimized = !old;
                                });
                            }
                            PAUSE_HIDDEN => {
                                let old = settings(app).pause_media_when_hidden;
                                persist_toggle(app, &pause_hidden, old, |value| {
                                    value.pause_media_when_hidden = !old;
                                });
                            }
                            NOTIFICATIONS => {
                                let old = settings(app).allow_notifications;
                                apply_permission(
                                    app,
                                    &notifications,
                                    old,
                                    COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                                    |value| value.allow_notifications = !old,
                                );
                            }
                            CAMERA => {
                                let old = settings(app).allow_camera;
                                apply_permission(
                                    app,
                                    &camera,
                                    old,
                                    COREWEBVIEW2_PERMISSION_KIND_CAMERA,
                                    |value| value.allow_camera = !old,
                                );
                            }
                            MICROPHONE => {
                                let old = settings(app).allow_microphone;
                                apply_permission(
                                    app,
                                    &microphone,
                                    old,
                                    COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
                                    |value| value.allow_microphone = !old,
                                );
                            }
                            RELOAD => {
                                if let Some(window) = app.get_webview_window(MAIN) {
                                    if let Err(error) = window.reload() {
                                        eprintln!("failed to reload main window: {error}");
                                    }
                                }
                            }
                            CLEAR_SITE_DATA => clear_site_data(app),
                            CHECK_UPDATE => check_for_updates(app),
                            GITHUB => {
                                if let Err(error) =
                                    tauri_plugin_opener::open_url(GITHUB_URL, None::<&str>)
                                {
                                    eprintln!("failed to open GitHub URL: {error}");
                                }
                            }
                            QUIT => app.exit(0),
                            _ => {}
                        }
                    })
                    .on_tray_icon_event(|tray, event| {
                        if matches!(
                            event,
                            TrayIconEvent::Click {
                                button: MouseButton::Left,
                                button_state: MouseButtonState::Up,
                                ..
                            }
                        ) {
                            show_main(tray.app_handle());
                        }
                    })
                    .build(app)?;

                let window = WebviewWindowBuilder::new(
                    app,
                    MAIN,
                    WebviewUrl::External(TIKTOK_URL.parse().unwrap()),
                )
                .title("TikTok Lite")
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 520.0)
                .visible(!initial.start_minimized)
                .initialization_script(media_control_script(initial.start_minimized))
                .on_navigation(move |url| match classify_navigation(url.as_str()) {
                    NavigationAction::InApp => true,
                    NavigationAction::ExternalHttps => {
                        open_external(url.as_str());
                        false
                    }
                    NavigationAction::Blocked => {
                        eprintln!("blocked navigation: {url}");
                        false
                    }
                })
                .on_new_window(move |url, _| match classify_popup_navigation(url.as_str()) {
                    PopupNavigationAction::InPopup => NewWindowResponse::Allow,
                    PopupNavigationAction::ExternalHttps => {
                        open_external(url.as_str());
                        NewWindowResponse::Deny
                    }
                    PopupNavigationAction::Blocked => {
                        eprintln!("blocked new-window URL: {url}");
                        NewWindowResponse::Deny
                    }
                })
                .build()?;

                let permission_app = app.handle().clone();
                window.with_webview(move |webview| unsafe {
                    let Ok(core) = webview.controller().CoreWebView2() else {
                        eprintln!("failed to access WebView2 permissions");
                        return;
                    };
                    let handler =
                        PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                            let Some(args) = args else {
                                return Ok(());
                            };
                            let mut uri = Default::default();
                            args.Uri(&mut uri)?;
                            if !permission_origin_allowed(&take_pwstr(uri)) {
                                args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                                return Ok(());
                            }

                            let mut kind = Default::default();
                            args.PermissionKind(&mut kind)?;
                            let current = settings(&permission_app);
                            let allowed = match kind {
                                COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => {
                                    current.allow_notifications
                                }
                                COREWEBVIEW2_PERMISSION_KIND_CAMERA => current.allow_camera,
                                COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => current.allow_microphone,
                                COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => false,
                                _ => false,
                            };
                            args.SetState(if allowed {
                                COREWEBVIEW2_PERMISSION_STATE_ALLOW
                            } else {
                                COREWEBVIEW2_PERMISSION_STATE_DENY
                            })?;
                            Ok(())
                        }));
                    let mut token = 0;
                    if let Err(error) = core.add_PermissionRequested(&handler, &mut token) {
                        eprintln!("failed to configure WebView2 permissions: {error}");
                    }
                })?;
                sync_permissions(app.handle());

                let event_app = app.handle().clone();
                window.on_window_event(move |event| match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        let current = settings(&event_app);
                        match lifecycle_action(WindowState::CloseRequested, &current) {
                            LifecycleAction::HideAndPause | LifecycleAction::Hide => {
                                api.prevent_close();
                                hide_main(&event_app);
                            }
                            LifecycleAction::Exit | LifecycleAction::Show => {}
                        }
                    }
                    WindowEvent::Resized(_) => {
                        if let Some(window) = event_app.get_webview_window(MAIN) {
                            match window.is_minimized() {
                                Ok(is_minimized) => {
                                    let was_minimized = event_app
                                        .state::<State>()
                                        .was_minimized
                                        .swap(is_minimized, Ordering::AcqRel);
                                    if is_minimized && settings(&event_app).pause_media_when_hidden {
                                        set_media_paused(&event_app, true);
                                    } else if restored_from_minimized(was_minimized, is_minimized) {
                                        set_media_paused(&event_app, false);
                                    }
                                }
                                Err(error) => eprintln!("failed to read minimized state: {error}"),
                            }
                        }
                    }
                    _ => {}
                });
                Ok(())
            })
            .run(tauri::generate_context!())
            .expect("failed to run TikTok Lite");
    }
}

#[cfg(windows)]
fn main() {
    windows_app::run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_contract_uses_exact_url_and_menu_ids() {
        assert_eq!(TIKTOK_URL, "https://www.tiktok.com/");
        assert_eq!(
            MENU_IDS,
            [
                "open",
                "pause_media",
                "settings_close_tray",
                "settings_autostart",
                "settings_start_minimized",
                "settings_pause_hidden",
                "permission_notifications",
                "permission_camera",
                "permission_microphone",
                "reload",
                "clear_site_data",
                "check_update",
                "github",
                "quit",
            ]
        );
    }
}

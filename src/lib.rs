use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use url::Url;

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
    Tray,
    Exit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub close_behavior: CloseBehavior,
    pub autostart: bool,
    pub start_minimized: bool,
    pub pause_media_when_hidden: bool,
    pub allow_notifications: bool,
    pub allow_camera: bool,
    pub allow_microphone: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_behavior: CloseBehavior::Tray,
            autostart: false,
            start_minimized: false,
            pause_media_when_hidden: true,
            allow_notifications: false,
            allow_camera: false,
            allow_microphone: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowState {
    CloseRequested,
    Hidden,
    Minimized,
    Visible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleAction {
    HideAndPause,
    Hide,
    Exit,
    Show,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NavigationAction {
    InApp,
    ExternalHttps,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupNavigationAction {
    InPopup,
    ExternalHttps,
    Blocked,
}

pub fn load_settings(path: &Path) -> Settings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_settings(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    save_settings_with(path, settings, replace_file)
}

fn save_settings_with<F>(path: &Path, settings: &Settings, replace: F) -> io::Result<()>
where
    F: FnOnce(&Path, &Path) -> io::Result<()>,
{
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let (temporary, mut file) = create_temporary(path)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn create_temporary(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    loop {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), id));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    if !path.exists() {
        return fs::rename(temporary, path);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut std::ffi::c_void,
            reserved: *mut std::ffi::c_void,
        ) -> i32;
    }

    let replaced: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let replacement: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Both paths are NUL-terminated, and optional pointers are null.
    let replaced = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn classify_navigation(input: &str) -> NavigationAction {
    let Ok(url) = Url::parse(input) else {
        return NavigationAction::Blocked;
    };
    if url.scheme() != "https" {
        return NavigationAction::Blocked;
    }
    match url.host_str() {
        Some("tiktok.com") => NavigationAction::InApp,
        Some(host) if host.ends_with(".tiktok.com") => NavigationAction::InApp,
        Some(_) => NavigationAction::ExternalHttps,
        None => NavigationAction::Blocked,
    }
}

pub fn permission_origin_allowed(input: &str) -> bool {
    Url::parse(input).is_ok_and(|url| {
        url.scheme() == "https" && url.host_str() == Some("www.tiktok.com") && url.port().is_none()
    })
}

pub fn classify_popup_navigation(input: &str) -> PopupNavigationAction {
    let Ok(url) = Url::parse(input) else {
        return PopupNavigationAction::Blocked;
    };
    if url.scheme() != "https" {
        return PopupNavigationAction::Blocked;
    }
    match url.host_str() {
        Some("tiktok.com" | "accounts.google.com" | "www.facebook.com" | "appleid.apple.com") => {
            PopupNavigationAction::InPopup
        }
        Some(host) if host.ends_with(".tiktok.com") => PopupNavigationAction::InPopup,
        Some(_) => PopupNavigationAction::ExternalHttps,
        None => PopupNavigationAction::Blocked,
    }
}

pub fn restored_from_minimized(was_minimized: bool, is_minimized: bool) -> bool {
    was_minimized && !is_minimized
}

pub fn lifecycle_action(state: WindowState, settings: &Settings) -> LifecycleAction {
    if state == WindowState::CloseRequested && settings.close_behavior == CloseBehavior::Exit {
        return LifecycleAction::Exit;
    }
    match state {
        WindowState::CloseRequested | WindowState::Hidden | WindowState::Minimized => {
            if settings.pause_media_when_hidden {
                LifecycleAction::HideAndPause
            } else {
                LifecycleAction::Hide
            }
        }
        WindowState::Visible => LifecycleAction::Show,
    }
}

pub fn media_control_script(paused: bool) -> String {
    format!(
        r#"(() => {{
  const state = window.__tiktokLiteMediaState ||= {{ paused: false }};
  state.paused = {paused};
  const pauseMedia = (root) => {{
    if (!state.paused) return;
    root.querySelectorAll?.('video, audio').forEach((media) => media.pause());
    if (root.matches?.('video, audio')) root.pause();
  }};
  if (!window.__tiktokLiteMediaObserver) {{
    window.__tiktokLiteMediaObserver = new MutationObserver((mutations) => {{
      mutations.forEach((mutation) => mutation.addedNodes.forEach(pauseMedia));
    }});
    window.__tiktokLiteMediaObserver.observe(document.documentElement, {{ childList: true, subtree: true }});
  }}
  if (!window.__tiktokLiteMediaPlayListener) {{
    window.__tiktokLiteMediaPlayListener = (event) => {{
      if (state.paused && event.target.matches?.('video, audio')) event.target.pause();
    }};
    document.addEventListener('play', window.__tiktokLiteMediaPlayListener, true);
  }}
  pauseMedia(document);
}})();"#
    )
}

pub fn release_version_is_newer(release: &str, current: &str) -> bool {
    fn parse(version: &str) -> Option<[u64; 3]> {
        let version = version.strip_prefix('v').unwrap_or(version);
        let mut parts = version.split('.');
        let parsed = [
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ];
        parts.next().is_none().then_some(parsed)
    }

    matches!((parse(release), parse(current)), (Some(release), Some(current)) if release > current)
}

pub fn update_error_message(status: Option<u16>) -> &'static str {
    match status {
        Some(404) => "Release not found. The repository may have no published release.",
        Some(403 | 429) => "GitHub temporarily rejected the update check. Please try again later.",
        Some(_) => "GitHub could not complete the update check. Please try again later.",
        None => "Could not reach GitHub. Check your internet connection and try again.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tiktok-lite-{name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn compares_github_release_versions() {
        assert!(release_version_is_newer("v0.2.0", "0.1.9"));
        assert!(release_version_is_newer("1.0.0", "0.9.9"));
        assert!(!release_version_is_newer("v0.1.0", "0.1.0"));
        assert!(!release_version_is_newer("v0.0.9", "0.1.0"));
        assert!(!release_version_is_newer("nightly", "0.1.0"));
        assert!(!release_version_is_newer("v1.0", "0.1.0"));
        assert!(!release_version_is_newer("v1.0.0-beta", "0.1.0"));
    }

    #[test]
    fn maps_update_failures_to_user_messages() {
        assert!(update_error_message(Some(404)).contains("no published release"));
        assert!(update_error_message(Some(429)).contains("temporarily rejected"));
        assert!(update_error_message(None).contains("internet connection"));
    }

    #[test]
    fn settings_defaults_are_safe() {
        let settings = Settings::default();
        assert_eq!(settings.close_behavior, CloseBehavior::Tray);
        assert!(settings.pause_media_when_hidden);
        assert!(!settings.autostart);
        assert!(!settings.start_minimized);
        assert!(!settings.allow_notifications);
        assert!(!settings.allow_camera);
        assert!(!settings.allow_microphone);
    }

    #[test]
    fn restricts_permission_requests_to_exact_tiktok_origin() {
        assert!(permission_origin_allowed("https://www.tiktok.com/"));
        assert!(permission_origin_allowed("https://www.tiktok.com/live"));
        assert!(!permission_origin_allowed("https://tiktok.com/"));
        assert!(!permission_origin_allowed("https://m.tiktok.com/"));
        assert!(!permission_origin_allowed(
            "https://www.tiktok.com.evil.test/"
        ));
        assert!(!permission_origin_allowed("http://www.tiktok.com/"));
        assert!(!permission_origin_allowed("https://www.tiktok.com:8443/"));
    }

    #[test]
    fn old_settings_gain_safe_permission_defaults() {
        let json = br#"{
            "close_behavior": "tray",
            "autostart": true,
            "start_minimized": false,
            "pause_media_when_hidden": true
        }"#;
        let settings: Settings = serde_json::from_slice(json).unwrap();
        assert!(settings.autostart);
        assert!(!settings.allow_notifications);
        assert!(!settings.allow_camera);
        assert!(!settings.allow_microphone);
    }

    #[test]
    fn navigation_policy_allows_only_https_tiktok_hosts_in_app() {
        assert_eq!(
            classify_navigation("https://www.tiktok.com/foryou"),
            NavigationAction::InApp
        );
        assert_eq!(
            classify_navigation("https://accounts.tiktok.com/login"),
            NavigationAction::InApp
        );
        assert_eq!(
            classify_navigation("https://example.com/video"),
            NavigationAction::ExternalHttps
        );
        assert_eq!(
            classify_navigation("javascript:alert(1)"),
            NavigationAction::Blocked
        );
        assert_eq!(
            classify_navigation("https://not-tiktok.com/"),
            NavigationAction::ExternalHttps
        );
        assert_eq!(
            classify_navigation("http://www.tiktok.com/"),
            NavigationAction::Blocked
        );
    }

    #[test]
    fn popup_policy_allows_only_tiktok_and_required_oauth_hosts() {
        for url in [
            "https://www.tiktok.com/login",
            "https://accounts.tiktok.com/login",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://www.facebook.com/v20.0/dialog/oauth",
            "https://appleid.apple.com/auth/authorize",
        ] {
            assert_eq!(
                classify_popup_navigation(url),
                PopupNavigationAction::InPopup
            );
        }
        assert_eq!(
            classify_popup_navigation("https://example.com/"),
            PopupNavigationAction::ExternalHttps
        );
        for url in ["http://accounts.google.com/", "javascript:alert(1)"] {
            assert_eq!(
                classify_popup_navigation(url),
                PopupNavigationAction::Blocked
            );
        }
    }

    #[test]
    fn media_resumes_only_after_minimized_to_restored_transition() {
        assert!(!restored_from_minimized(false, false));
        assert!(!restored_from_minimized(false, true));
        assert!(restored_from_minimized(true, false));
        assert!(!restored_from_minimized(true, true));
    }

    #[test]
    fn close_request_uses_configured_behavior_and_pause_setting() {
        assert_eq!(
            lifecycle_action(WindowState::CloseRequested, &Settings::default()),
            LifecycleAction::HideAndPause
        );

        let no_pause = Settings {
            pause_media_when_hidden: false,
            ..Settings::default()
        };
        assert_eq!(
            lifecycle_action(WindowState::CloseRequested, &no_pause),
            LifecycleAction::Hide
        );

        let exit = Settings {
            close_behavior: CloseBehavior::Exit,
            ..Settings::default()
        };
        assert_eq!(
            lifecycle_action(WindowState::CloseRequested, &exit),
            LifecycleAction::Exit
        );
    }

    #[test]
    fn hidden_window_pauses_only_when_enabled() {
        assert_eq!(
            lifecycle_action(WindowState::Hidden, &Settings::default()),
            LifecycleAction::HideAndPause
        );
        assert_eq!(
            lifecycle_action(WindowState::Visible, &Settings::default()),
            LifecycleAction::Show
        );
    }

    #[test]
    fn media_script_pauses_without_forcing_playback() {
        let paused = media_control_script(true);
        assert!(paused.contains("media.pause()"));
        assert!(paused.contains("MutationObserver"));
        assert!(paused.contains("if (!window.__tiktokLiteMediaPlayListener)"));
        assert!(paused.contains("if (state.paused && event.target.matches?.('video, audio'))"));
        assert_eq!(paused.matches("addEventListener('play'").count(), 1);
        assert!(paused.contains("window.__tiktokLiteMediaPlayListener, true)"));
        assert!(!paused.contains("media.play()"));

        let resumed = media_control_script(false);
        assert!(!resumed.contains("media.play()"));
    }

    #[test]
    fn settings_round_trip_as_json() {
        let path = temp_path("round-trip");
        let settings = Settings {
            close_behavior: CloseBehavior::Exit,
            autostart: true,
            start_minimized: true,
            pause_media_when_hidden: false,
            allow_notifications: true,
            allow_camera: true,
            allow_microphone: true,
        };

        save_settings(&path, &settings).unwrap();
        assert_eq!(load_settings(&path), settings);
        let json: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["close_behavior"], "exit");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let root = temp_path("missing-parent").with_extension("");
        let path = root.join("nested/settings.json");
        save_settings(&path, &Settings::default()).unwrap();
        assert_eq!(load_settings(&path), Settings::default());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_file_uses_defaults() {
        let path = temp_path("missing");
        assert_eq!(load_settings(&path), Settings::default());
    }

    #[test]
    fn corrupt_file_uses_defaults() {
        let path = temp_path("corrupt");
        fs::write(&path, b"not json").unwrap();
        assert_eq!(load_settings(&path), Settings::default());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_save_replaces_file_with_valid_json() {
        let path = temp_path("atomic");
        fs::write(&path, br#"{"close_behavior":"tray","autostart":false,"start_minimized":false,"pause_media_when_hidden":true}"#).unwrap();
        let settings = Settings {
            autostart: true,
            ..Settings::default()
        };

        save_settings(&path, &settings).unwrap();

        let bytes = fs::read(&path).unwrap();
        let saved: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved, settings);
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_replacement_preserves_old_valid_json() {
        let path = temp_path("failed-replacement");
        let old = Settings::default();
        let new = Settings {
            autostart: true,
            ..Settings::default()
        };
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();

        let error = save_settings_with(&path, &new, |_, _| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked"))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(load_settings(&path), old);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_does_not_claim_or_overwrite_fixed_temporary_path() {
        let path = temp_path("temp-ownership");
        let mut fixed_temporary = path.as_os_str().to_owned();
        fixed_temporary.push(".tmp");
        let fixed_temporary = std::path::PathBuf::from(fixed_temporary);
        fs::write(&fixed_temporary, b"owned by another save").unwrap();

        save_settings(&path, &Settings::default()).unwrap();

        assert_eq!(
            fs::read(&fixed_temporary).unwrap(),
            b"owned by another save"
        );
        assert_eq!(load_settings(&path), Settings::default());
        fs::remove_file(path).unwrap();
        fs::remove_file(fixed_temporary).unwrap();
    }
}

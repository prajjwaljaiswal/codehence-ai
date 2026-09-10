use log::{debug, warn};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

/// Show a desktop notification.
///
/// Fired from the Rust side rather than the webview so it needs no JS
/// notification-permission plumbing; macOS prompts the user for permission
/// the first time the app posts one.
#[tauri::command]
pub async fn show_desktop_notification(
    app: AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    debug!("Showing notification: {}", title);

    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| format!("Failed to show notification: {}", e))
}

/// Set the app icon's badge count (the dock badge on macOS, the taskbar
/// counter on Linux). Pass `None` — or 0 — to clear it.
#[tauri::command]
pub async fn set_app_badge_count(app: AppHandle, count: Option<i64>) -> Result<(), String> {
    // 0 reads as "nothing pending", which should clear the badge rather than
    // render a literal "0".
    let count = count.filter(|c| *c > 0);
    debug!("Setting badge count: {:?}", count);

    // The main window carries the badge. The label isn't configured in
    // tauri.conf.json (so it defaults to "main"), but fall back to whatever
    // window exists rather than depending on that default.
    let window = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().values().next().cloned());

    match window {
        Some(window) => window
            .set_badge_count(count)
            .map_err(|e| format!("Failed to set badge count: {}", e)),
        None => {
            warn!("No window available to set the badge count on");
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
use crate::app::APP_ID;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn init_notifications() {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = notify_rust::set_application(APP_ID) {
            log::error!("Failed to set notification application: {error}");
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn post_limit_notification(title: &str, body: &str) {
    let result = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show();
    if let Err(error) = result {
        log::error!("Failed to deliver notification: {error}");
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn init_notifications() {}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) fn post_limit_notification(title: &str, body: &str) {
    let _ = (title, body);
}

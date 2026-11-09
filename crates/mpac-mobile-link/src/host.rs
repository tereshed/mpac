#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "../../../desktop/src-tauri/src/connection.rs"]
#[allow(dead_code)]
mod connection;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod host_service;
#[cfg(target_os = "macos")]
mod telegram;
fn main() -> anyhow::Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    return if std::env::args().nth(1).as_deref() == Some("admin") {
        host_service::admin_cli()
    } else {
        host_service::run()
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("The companion host requires macOS or Linux")
}

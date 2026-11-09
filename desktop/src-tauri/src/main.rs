#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod connection;
mod gateway;
mod phone;
mod phone_service;
mod settings;
mod terminal_bridge;

use anyhow::{Context, Result};
use connection::{Connection, DesktopConfig, NativeClient};
use gateway::Gateway;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

struct NativeState {
    profile: PathBuf,
    override_dir: Mutex<Option<PathBuf>>,
    gateway: Arc<Gateway>,
    busy: AtomicBool,
    sidecar: PathBuf,
    ui: PathBuf,
    _terminal_bridge: Option<terminal_bridge::Bridge>,
    phone: Arc<phone::Phone>,
}

fn resource_paths(executable: &Path) -> Result<(PathBuf, PathBuf)> {
    let directory = executable
        .parent()
        .context("Cannot locate application executable")?;
    let sidecar = directory.join(if cfg!(windows) {
        "mpac-daemon.exe"
    } else {
        "mpac-daemon"
    });
    #[cfg(target_os = "macos")]
    let ui = directory
        .parent()
        .context("Cannot locate app resources")?
        .join("Resources/ui");
    #[cfg(not(target_os = "macos"))]
    let ui = directory.join("ui");
    if cfg!(debug_assertions) && !ui.join("index.html").exists() {
        return Ok((
            sidecar,
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/ui"),
        ));
    }
    Ok((sidecar, ui))
}

fn is_launcher(url: &url::Url) -> bool {
    let local = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    local
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && matches!(url.path(), "" | "/" | "/index.html")
}

fn connect(state: &NativeState) -> Result<(Connection, DesktopConfig)> {
    let chosen = state.override_dir.lock().unwrap().clone();
    let settings = settings::load(&state.profile, chosen.as_deref())?;
    let config = DesktopConfig {
        data_dir: settings.data_dir.clone(),
    };
    let connection = connection::connect_or_start(&config, &state.sidecar, &state.ui)?;
    settings::save(&state.profile, &settings.data_dir.canonicalize()?)?;
    *state.override_dir.lock().unwrap() = None;
    Ok((connection, config))
}

fn open_connection(app: tauri::AppHandle) {
    if app.state::<NativeState>().busy.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let result = connect(&app.state::<NativeState>()).and_then(|(connection, config)| {
            let data = config.data_dir.clone();
            // Recover only the configured launchd gateway; never restart agents.
            let _ = app.state::<NativeState>().phone.restore(&data);
            Ok(phone::DesktopTransport {
                client: NativeClient::new(config, connection)?,
                data,
                phone: app.state::<NativeState>().phone.clone(),
                app: app.clone(),
            })
        });
        let target = match result {
            Ok(client) => app.state::<NativeState>().gateway.install(Arc::new(client)),
            Err(error) => {
                app.state::<NativeState>().gateway.clear();
                // Native errors never contain the authenticated launch URL.
                let mut url = url::Url::parse("tauri://localhost/index.html").unwrap();
                url.set_fragment(Some(
                    &url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("error", &format!("{error:#}"))
                        .finish(),
                ));
                url
            }
        };
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.navigate(target);
        }
        app.state::<NativeState>()
            .busy
            .store(false, Ordering::Release);
    });
}

fn native_main(options: settings::Options) -> Result<()> {
    let (sidecar, ui) = resource_paths(&std::env::current_exe()?)?;
    let app = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("mpac", |context, request, responder| {
            if context.webview_label() != "main" {
                responder.respond(gateway::error(403, "Unexpected workspace window"));
                return;
            }
            let gateway = context.app_handle().state::<NativeState>().gateway.clone();
            let Some(permit) = gateway.permit() else {
                responder.respond(gateway::error(503, "Workspace is busy; try again"));
                return;
            };
            std::thread::spawn(move || {
                responder.respond(gateway.serve(request));
                drop(permit);
            });
        })
        .setup(move |app| {
            let profile = options
                .profile_dir
                .clone()
                .unwrap_or(app.path().app_config_dir()?);
            let gateway = Arc::new(Gateway::new(ui.clone()));
            let terminal_bridge = terminal_bridge::Bridge::start(gateway.clone())?;
            let phone = Arc::new(phone::Phone::new(
                profile.clone(),
                sidecar.with_file_name("mpac-mobile-host"),
            ));
            phone.watch(app.handle().clone());
            app.manage(NativeState {
                profile,
                override_dir: Mutex::new(options.data_dir.clone()),
                gateway,
                busy: AtomicBool::new(false),
                sidecar: sidecar.clone(),
                ui: ui.clone(),
                _terminal_bridge: Some(terminal_bridge),
                phone,
            });
            let native_menu = tauri::menu::Menu::default(app.handle())?;
            let workspace_menu = tauri::menu::SubmenuBuilder::new(app, "Workspace")
                .text("reconnect", "Reconnect")
                .text("show-workspace", "Show workspace")
                .build()?;
            native_menu.append(&workspace_menu)?;
            app.set_menu(native_menu)?;
            app.on_menu_event(|app, event| match event.id().as_ref() {
                "reconnect" => open_connection(app.clone()),
                "show-workspace" => {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            });
            let navigation_app = app.handle().clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("MPAC")
                .inner_size(1280.0, 850.0)
                .min_inner_size(380.0, 480.0)
                .center()
                .disable_drag_drop_handler()
                .devtools(false)
                .on_navigation(move |url| {
                    if is_launcher(url) {
                        return true;
                    }
                    gateway::workspace_url(url)
                        && navigation_app.state::<NativeState>().gateway.ready()
                })
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .on_download(|_, _| false)
                .on_page_load(|window, payload| {
                    if payload.event() == PageLoadEvent::Finished
                        && is_launcher(payload.url())
                        && payload.url().fragment().is_none()
                    {
                        open_connection(window.app_handle().clone());
                    }
                })
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())?;
    #[cfg(target_os = "macos")]
    prevent_idle_sleep();
    app.run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            app.state::<NativeState>().phone.shutdown();
        }
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = event
            && let Some(window) = app.get_webview_window("main")
        {
            let _ = window.show();
            let _ = window.set_focus();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (app, event);
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn prevent_idle_sleep() {
    use std::process::{Command, Stdio};

    // Closing the window hides it. Watch the desktop PID so the assertion is
    // released on Quit or a crash, independently of the daemon and agents.
    match Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => eprintln!("Could not prevent macOS idle sleep: {error}"),
    }
}

fn run() -> Result<()> {
    let options = settings::Options::parse(std::env::args().skip(1))?;
    if options.help {
        println!(
            "MPAC desktop [--data-dir PATH] [--profile-dir PATH] [--probe]\nWindow close and Quit leave the local service and CLI sessions running."
        );
        return Ok(());
    }
    if options.probe {
        let profile = options
            .profile_dir
            .context("--probe requires an isolated --profile-dir")?;
        let (sidecar, ui) = resource_paths(&std::env::current_exe()?)?;
        let state = NativeState {
            profile,
            override_dir: Mutex::new(options.data_dir),
            gateway: Arc::new(Gateway::new(ui.clone())),
            busy: AtomicBool::new(false),
            sidecar,
            ui,
            _terminal_bridge: None,
            phone: Arc::new(phone::Phone::new(PathBuf::new(), PathBuf::new())),
        };
        let (connection, config) = connect(&state)?;
        let metadata = (connection.pid, connection.port, connection.reused);
        NativeClient::new(config, connection)?.request("GET", "/api/state", &[])?;
        println!(
            "{}",
            serde_json::json!({"pid":metadata.0,"port":metadata.1,"reused":metadata.2})
        );
        return Ok(());
    }
    native_main(options)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("MPAC desktop could not start: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_navigation_cannot_be_redirected_to_other_schemes_or_hosts() {
        // Tauri strips index.html from the initial custom-scheme URL.
        assert!(is_launcher(&"tauri://localhost".parse().unwrap()));
        for allowed in [
            "tauri://localhost/index.html",
            "tauri://localhost/index.html#error=safe",
            "http://tauri.localhost/",
        ] {
            assert!(is_launcher(&allowed.parse().unwrap()));
        }
        for denied in [
            "file:///index.html",
            "https://localhost/index.html",
            "http://tauri.localhost:123/",
            "tauri://localhost/other.html",
            "tauri://user@localhost/index.html",
        ] {
            assert!(!is_launcher(&denied.parse().unwrap()));
        }
    }
}

//! Shared mobile controller. Secret profiles only cross the OS credential-store
//! boundary. Renderer requests cannot supply an address, trust root, or key.
use anyhow::{Context, Result, ensure};
use mpac_mobile_link::{Offer, Profile, RemoteClient, Reply, secret};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;
mod incoming;

#[derive(Default)]
struct MobileState {
    client: Option<Arc<RemoteClient>>,
    pending: Option<Arc<RemoteClient>>,
    generation: u64,
}
type Shared = Arc<Mutex<MobileState>>;
// URL delivery and acknowledgement run on the UI thread. Their lock must never
// be shared with network-client destruction or credential-store operations.
type IncomingShared = Arc<Mutex<incoming::Inbox>>;
fn capture_link(inbox: &IncomingShared, value: &str) {
    inbox.lock().unwrap().receive(value);
}
fn retire<T: Send + 'static>(value: Option<T>) {
    if let Some(value) = value {
        // reqwest's blocking client can join its network runtime on Drop.
        // Cancellation publishes immediately; cleanup must not hold any lock.
        tauri::async_runtime::spawn_blocking(move || drop(value));
    }
}

fn entry() -> Result<keyring_core::Entry> {
    #[cfg(target_os = "ios")]
    let store = apple_native_keyring_store::protected::Store::new()?;
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new()?;
    #[cfg(target_os = "android")]
    let store = android_native_keyring_store::Store::new_with_configuration(
        &std::collections::HashMap::from([("name", "mpac-mobile")]),
    )?;
    use keyring_core::api::CredentialStoreApi;
    Ok(store.build("com.mpac.mobile", "computer", None)?)
}
fn error(_: impl std::fmt::Display) -> String {
    "Secure connection failed. Check the computer, pairing approval and network; no command was retried.".into()
}
fn saved() -> Result<Option<Profile>> {
    match entry()?.get_password() {
        Ok(value) => Ok(Some(
            serde_json::from_str(&value).context("Invalid saved connection")?,
        )),
        Err(keyring_core::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(error)?
        .map_err(error)
}

#[tauri::command]
async fn voice_request(
    state: State<'_, Arc<mpac_voice::Voice>>,
    method: String,
    operation: String,
    body: Option<String>,
) -> Result<Value, String> {
    let voice = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        voice.request(
            &method,
            &operation,
            body.as_deref().unwrap_or("").as_bytes(),
        )
    })
    .await
    .map_err(|_| "voice_unavailable".to_owned())?
    .map_err(str::to_owned)
}

#[tauri::command]
async fn mobile_status(state: State<'_, Shared>) -> Result<Value, String> {
    let state = state.inner().clone();
    blocking(move || {
        if let Some(client) = &state.lock().unwrap().client {
            return Ok(json!({"connected":true,"endpoint":client.profile().endpoint}));
        }
        let generation = state.lock().unwrap().generation;
        let Some(profile) = saved()? else {
            return Ok(json!({"connected":false}));
        };
        let endpoint = profile.endpoint.clone();
        let client = Arc::new(RemoteClient::new(profile)?);
        let mut state = state.lock().unwrap();
        ensure!(state.generation == generation, "Connection changed");
        let previous = state.client.replace(client);
        drop(state);
        retire(previous);
        Ok(json!({"connected":true,"endpoint":endpoint}))
    })
    .await
}
#[tauri::command]
async fn mobile_take_link(
    app: tauri::AppHandle,
    state: State<'_, IncomingShared>,
) -> Result<Option<incoming::Delivery>, String> {
    let state = state.inner().clone();
    blocking(move || {
        let hydrate = {
            let mut state = state.lock().unwrap();
            if state.hydrated {
                None
            } else {
                state.hydrated = true;
                Some(state.sequence)
            }
        };
        if let Some(sequence) = hydrate {
            let urls = match app.deep_link().get_current() {
                Ok(urls) => urls,
                Err(e) => {
                    state.lock().unwrap().hydrated = false;
                    return Err(e.into());
                }
            };
            let mut state = state.lock().unwrap();
            // A warm event received during startup takes precedence.
            if state.sequence == sequence {
                for url in urls.unwrap_or_default() {
                    state.receive(url.as_str());
                }
            }
        }
        Ok(state.lock().unwrap().peek())
    })
    .await
}
#[tauri::command]
fn mobile_acknowledge_link(state: State<'_, IncomingShared>, sequence: u64) {
    state.lock().unwrap().acknowledge(sequence);
}
#[tauri::command]
async fn mobile_begin(
    state: State<'_, Shared>,
    link: String,
    name: String,
) -> Result<Value, String> {
    let state = state.inner().clone();
    blocking(move || {
        let offer = Offer::parse_input(link.trim())?;
        let (generation, previous) = {
            let mut state = state.lock().unwrap();
            state.generation += 1;
            (state.generation, state.pending.take())
        };
        retire(previous);
        let mut profile = Profile {
            endpoint: offer.endpoint,
            certificate: offer.certificate,
            device_key: secret(),
            device_id: String::new(),
            computer_name: "MPAC computer".into(),
        };
        let client = RemoteClient::new(profile.clone())?;
        let reply = client.pairing_request(
            "/pair/request",
            &serde_json::to_vec(&json!({"invite":offer.invite,"name":name}))?,
        )?;
        ensure!(
            reply.status == 200,
            "Pairing invitation expired or already used"
        );
        let response: Value = serde_json::from_str(&reply.body)?;
        profile.device_id = response["id"]
            .as_str()
            .context("Missing device ID")?
            .to_owned();
        let pending = Arc::new(RemoteClient::new(profile)?);
        let mut state = state.lock().unwrap();
        ensure!(state.generation == generation, "Pairing cancelled");
        let previous = state.pending.replace(pending);
        drop(state);
        retire(previous);
        Ok(json!({"code":response["code"],"expires_at":response["expires_at"]}))
    })
    .await
}
#[tauri::command]
async fn mobile_pair_status(state: State<'_, Shared>) -> Result<Value, String> {
    let state = state.inner().clone();
    blocking(move || {
        let (client, generation) = {
            let state = state.lock().unwrap();
            (
                state.pending.clone().context("No pairing request")?,
                state.generation,
            )
        };
        let reply = client.pairing_request(
            "/pair/status",
            &serde_json::to_vec(&json!({"id":client.profile().device_id}))?,
        )?;
        ensure!(reply.status == 200, "Pairing request failed");
        let response: Value = serde_json::from_str(&reply.body)?;
        if response["status"] == "approved" {
            let mut state = state.lock().unwrap();
            ensure!(state.generation == generation, "Pairing cancelled");
            entry()?.set_password(&serde_json::to_string(client.profile())?)?;
            let previous = state.client.replace(client);
            let pending = state.pending.take();
            drop(state);
            retire(previous);
            retire(pending);
        }
        Ok(json!({"status":response["status"]}))
    })
    .await
}
#[tauri::command]
async fn mobile_request(
    state: State<'_, Shared>,
    method: String,
    path: String,
    body: Option<String>,
) -> Result<Reply, String> {
    let state = state.inner().clone();
    blocking(move || {
        let client = state
            .lock()
            .unwrap()
            .client
            .clone()
            .context("No paired computer")?;
        client.request(&method, &path, body.as_deref().unwrap_or("").as_bytes())
    })
    .await
}
#[tauri::command]
async fn mobile_forget(state: State<'_, Shared>) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        let mut state = state.lock().unwrap();
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => {}
            Err(e) => return Err(e.into()),
        }
        state.generation += 1;
        let client = state.client.take();
        let pending = state.pending.take();
        drop(state);
        retire(client);
        retire(pending);
        Ok(())
    })
    .await
}
#[tauri::command]
async fn mobile_cancel(state: State<'_, Shared>) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        let mut state = state.lock().unwrap();
        state.generation += 1;
        let pending = state.pending.take();
        drop(state);
        retire(pending);
        Ok(())
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .manage(Shared::default())
        .manage(IncomingShared::default())
        .manage(Arc::new(mpac_voice::Voice::default()))
        .invoke_handler(tauri::generate_handler![
            voice_request,
            mobile_status,
            mobile_begin,
            mobile_pair_status,
            mobile_request,
            mobile_forget,
            mobile_cancel,
            mobile_take_link,
            mobile_acknowledge_link
        ])
        .setup(|app| {
            let state = app.state::<IncomingShared>().inner().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    capture_link(&state, url.as_str());
                }
            });
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("MPAC Mobile")
                .initialization_script("window.__MPAC_NATIVE_MOBILE__ = true;")
                .on_permission_request(|_, kind| {
                    use tauri::webview::{PermissionKind, PermissionResponse};
                    match kind {
                        PermissionKind::Microphone => PermissionResponse::Default,
                        PermissionKind::Camera => PermissionResponse::Deny,
                        _ => PermissionResponse::Default,
                    }
                })
                .on_navigation(bundled_navigation)
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Could not start MPAC Mobile")
        .run(|app, event| {
            // Capture the native event directly as well as the plugin event.
            // This does not depend on a webview listener being ready at launch.
            #[cfg(any(target_os = "ios", target_os = "macos"))]
            if let tauri::RunEvent::Opened { urls } = event {
                let state = app.state::<IncomingShared>();
                for url in urls {
                    capture_link(state.inner(), url.as_str());
                }
            }
            #[cfg(target_os = "android")]
            let _ = (app, event);
        });
}

fn bundled_navigation(url: &tauri::Url) -> bool {
    ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost")))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/" | "/index.html")
        && url.query().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_delivery_is_available_while_connection_state_is_locked() {
        use std::{sync::mpsc, time::Duration};
        let connection = Shared::default();
        let inbox = IncomingShared::default();
        let guard = connection.lock().unwrap();
        let (done, result) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            capture_link(&inbox, "mpac://open");
            let sequence = inbox.lock().unwrap().peek().unwrap().sequence;
            inbox.lock().unwrap().acknowledge(sequence);
            done.send(inbox.lock().unwrap().peek().is_none()).unwrap();
        });
        let available = result.recv_timeout(Duration::from_secs(1));
        drop(guard);
        worker.join().unwrap();
        assert_eq!(available.unwrap(), true);
    }
    #[test]
    fn retiring_a_stalled_network_runtime_does_not_wait_on_the_ui_command() {
        use std::{sync::mpsc, time::Duration};
        struct SlowDrop {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Drop for SlowDrop {
            fn drop(&mut self) {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        let (entered, started) = mpsc::channel();
        let (release, unblock) = mpsc::channel();
        let (done, returned) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            retire(Some(SlowDrop {
                entered,
                release: unblock,
            }));
            done.send(()).unwrap();
        });
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let immediate = returned.recv_timeout(Duration::from_millis(250)).is_ok();
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            immediate,
            "Cancel must publish before network cleanup finishes"
        );
    }
    #[test]
    fn navigation_cannot_load_another_local_service_with_native_privileges() {
        for allowed in [
            "tauri://localhost/",
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
        ] {
            assert!(bundled_navigation(&allowed.parse().unwrap()));
        }
        for denied in [
            "http://localhost/",
            "https://localhost/",
            "http://127.0.0.1/",
            "http://tauri.localhost:123/",
            "tauri://user@localhost/",
            "tauri://localhost/other.html",
            "tauri://localhost/?url=elsewhere",
        ] {
            assert!(!bundled_navigation(&denied.parse().unwrap()), "{denied}");
        }
    }
}

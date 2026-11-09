//! Device-local dictation. The workspace daemon and paired devices never
//! receive the provider credential or the recorded audio.
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::blocking::{Client, multipart};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
mod settings;
use settings::Settings;
use uuid::Uuid;

type Result<T> = std::result::Result<T, &'static str>;
const MAX_AUDIO: usize = 8 * 1024 * 1024;
const MAX_RESPONSE: u64 = 128 * 1024;

fn credential() -> Result<keyring_core::Entry> {
    #[cfg(any(target_os = "ios", target_os = "macos", target_os = "android"))]
    {
        use keyring_core::api::CredentialStoreApi;
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(target_os = "ios")]
        let store = apple_native_keyring_store::protected::Store::new();
        #[cfg(target_os = "android")]
        let store = android_native_keyring_store::Store::new_with_configuration(
            &std::collections::HashMap::from([("name", "mpac-voice")]),
        );
        store
            .and_then(|store| store.build("com.mpac.voice", "transcription", None))
            .map_err(|_| "voice_storage")
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos", target_os = "android")))]
    Err("voice_unavailable")
}

fn load() -> Result<Settings> {
    match credential()?.get_password() {
        Ok(value) => Settings::from_stored(&value),
        Err(keyring_core::Error::NoEntry) => Ok(Settings::default()),
        Err(_) => Err("voice_storage"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recording {
    id: Uuid,
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebAudio {
    revision: String,
    mime_type: String,
    data_base64: String,
}

#[derive(Default)]
pub struct Voice {
    settings: Mutex<()>,
    recording: Mutex<Option<Uuid>>,
    transcribing: AtomicBool,
}

struct RequestGuard<'a>(&'a AtomicBool);
impl Drop for RequestGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Voice {
    /// Called on background workers by the native client transports.
    pub fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<Value> {
        if body.len() > MAX_AUDIO.div_ceil(3) * 4 + 1024 {
            return Err("voice_audio_size");
        }
        match (method, path) {
            ("GET", "settings") => Ok(load()?.public()),
            ("POST", "settings") => {
                if body.len() > 8192 {
                    return Err("voice_invalid");
                }
                let _lock = self.settings.lock().unwrap();
                let settings = load()?.update(decode(body)?)?;
                let saved = serde_json::to_string(&settings).map_err(|_| "voice_storage")?;
                credential()?
                    .set_password(&saved)
                    .map_err(|_| "voice_storage")?;
                Ok(settings.public())
            }
            ("POST", "start") => {
                let request: Recording = decode(body)?;
                load()?.ready(&request.revision)?;
                {
                    let mut recording = self.recording.lock().unwrap();
                    if recording.take().is_some() {
                        capture::cancel();
                    }
                    *recording = Some(request.id);
                }
                let authorized = capture::authorize();
                let mut recording = self.recording.lock().unwrap();
                if *recording != Some(request.id) {
                    return Err("voice_cancelled");
                }
                if let Err(error) = authorized.and_then(|()| capture::start()) {
                    *recording = None;
                    return Err(error);
                }
                Ok(json!({}))
            }
            ("POST", "cancel") => {
                let request: Recording = decode(body)?;
                let mut recording = self.recording.lock().unwrap();
                if *recording == Some(request.id) {
                    *recording = None;
                    capture::cancel();
                }
                Ok(json!({}))
            }
            ("POST", "stop") => {
                let request: Recording = decode(body)?;
                let audio = {
                    let mut recording = self.recording.lock().unwrap();
                    if *recording != Some(request.id) {
                        return Err("voice_cancelled");
                    }
                    *recording = None;
                    capture::stop()?
                };
                self.transcribe(&request.revision, "audio/mp4", audio)
            }
            ("POST", "transcribe") if cfg!(target_os = "android") => {
                let request: WebAudio = decode(body)?;
                let audio = STANDARD
                    .decode(request.data_base64)
                    .map_err(|_| "voice_invalid")?;
                self.transcribe(&request.revision, &request.mime_type, audio)
            }
            _ => Err("voice_unavailable"),
        }
    }

    fn transcribe(&self, revision: &str, mime: &str, audio: Vec<u8>) -> Result<Value> {
        if audio.is_empty() {
            return Err("voice_no_audio");
        }
        if audio.len() > MAX_AUDIO {
            return Err("voice_audio_size");
        }
        let (filename, mime) = match mime.split(';').next().unwrap_or("") {
            "audio/mp4" | "audio/x-m4a" => ("recording.m4a", "audio/mp4"),
            "audio/webm" => ("recording.webm", "audio/webm"),
            _ => return Err("voice_audio_format"),
        };
        if self.transcribing.swap(true, Ordering::AcqRel) {
            return Err("voice_busy");
        }
        let _guard = RequestGuard(&self.transcribing);
        let settings = load()?;
        let (profile, url) = settings.ready(revision)?;
        let file = multipart::Part::bytes(audio)
            .file_name(filename)
            .mime_str(mime)
            .map_err(|_| "voice_audio_format")?;
        let form = multipart::Form::new()
            .text("model", profile.model.clone())
            .part("file", file);
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()
            .map_err(|_| "voice_network")?;
        let response = client
            .post(url)
            .bearer_auth(&profile.api_key)
            .multipart(form)
            .send()
            .map_err(|_| "voice_network")?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err("voice_auth"),
            429 => return Err("voice_rate"),
            400 | 404 | 415 | 422 => return Err("voice_provider_config"),
            _ => return Err("voice_provider"),
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "voice_network")?;
        if bytes.len() as u64 > MAX_RESPONSE {
            return Err("voice_response");
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "voice_response")?;
        let text = value["text"].as_str().ok_or("voice_response")?.trim();
        if text.is_empty() {
            return Err("voice_no_speech");
        }
        Ok(json!({"text": text}))
    }
}

fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|_| "voice_invalid")
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod capture {
    use super::Result;
    use std::ffi::c_void;

    unsafe extern "C" {
        fn mpac_voice_authorize() -> bool;
        fn mpac_voice_start() -> bool;
        fn mpac_voice_stop(length: *mut usize) -> *mut c_void;
        fn mpac_voice_cancel();
        fn free(value: *mut c_void);
    }
    pub fn authorize() -> Result<()> {
        unsafe { mpac_voice_authorize() }
            .then_some(())
            .ok_or("voice_permission")
    }
    pub fn start() -> Result<()> {
        unsafe { mpac_voice_start() }
            .then_some(())
            .ok_or("voice_capture")
    }
    pub fn stop() -> Result<Vec<u8>> {
        let mut length = 0;
        let bytes = unsafe { mpac_voice_stop(&mut length) };
        if bytes.is_null() {
            return Err("voice_no_audio");
        }
        // The recorder allocates exactly length bytes with malloc.
        let audio = unsafe { std::slice::from_raw_parts(bytes.cast(), length) }.to_vec();
        unsafe { free(bytes) };
        Ok(audio)
    }
    pub fn cancel() {
        unsafe { mpac_voice_cancel() }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod capture {
    use super::Result;
    pub fn authorize() -> Result<()> {
        Err("voice_unavailable")
    }
    pub fn start() -> Result<()> {
        Err("voice_unavailable")
    }
    pub fn stop() -> Result<Vec<u8>> {
        Err("voice_unavailable")
    }
    pub fn cancel() {}
}

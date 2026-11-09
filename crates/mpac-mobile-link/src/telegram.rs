//! Explicit, owner-bound delivery. No bot command can run a terminal action.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::VecDeque, io::Read, path::Path, time::Duration};

pub trait Vault: Send {
    fn load(&self) -> Result<Option<String>>;
    fn save(&self, value: &str) -> Result<()>;
    fn delete(&self) -> Result<()>;
}
struct Keychain(keyring_core::Entry);
impl Vault for Keychain {
    fn load(&self) -> Result<Option<String>> {
        match self.0.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => bail!("Could not read Telegram credentials from Keychain"),
        }
    }
    fn save(&self, value: &str) -> Result<()> {
        self.0
            .set_password(value)
            .map_err(|_| anyhow::anyhow!("Could not save Telegram credentials in Keychain"))
    }
    fn delete(&self) -> Result<()> {
        match self.0.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => bail!("Could not remove Telegram credentials from Keychain"),
        }
    }
}
pub trait BotApi: Send {
    fn call(&self, token: &str, method: &str, body: Value) -> Result<Value>;
}
struct TelegramApi(reqwest::blocking::Client);
impl BotApi for TelegramApi {
    fn call(&self, token: &str, method: &str, body: Value) -> Result<Value> {
        // Never propagate reqwest errors: their URL includes the bot token.
        let response = self
            .0
            .post(format!("https://api.telegram.org/bot{token}/{method}"))
            .json(&body)
            .send()
            .map_err(|_| {
                anyhow::anyhow!(
                    "Telegram did not confirm the request. Check the bot before trying again."
                )
            })?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow::anyhow!("Could not read Telegram response"))?;
        ensure!(
            bytes.len() <= 1024 * 1024,
            "Telegram response was too large"
        );
        let data: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Unexpected Telegram response"))?;
        if !status.is_success() || data["ok"] != true {
            bail!(
                "Telegram rejected the request. Check the token, bot permissions, and whether another service uses this bot."
            );
        }
        Ok(data["result"].clone())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    token: String,
    bot_id: i64,
    username: String,
    chat_id: i64,
    chat_label: String,
}
struct Pending {
    token: String,
    bot_id: i64,
    username: String,
    challenge: String,
    started: u64,
    offset: i64,
    candidate: Option<(i64, String)>,
}
pub struct Telegram {
    vault: Box<dyn Vault>,
    api: Box<dyn BotApi>,
    saved: Option<Saved>,
    pending: Option<Pending>,
    deliveries: VecDeque<(String, bool)>,
}
impl Telegram {
    pub fn open(directory: &Path) -> Result<Self> {
        use keyring_core::api::CredentialStoreApi;
        let store = apple_native_keyring_store::keychain::Store::new()
            .map_err(|_| anyhow::anyhow!("Keychain is unavailable"))?;
        let entry = store
            .build(
                "com.mpac.telegram",
                &mpac_mobile_link::secret_hash(&directory.to_string_lossy()),
                None,
            )
            .map_err(|_| anyhow::anyhow!("Keychain is unavailable"))?;
        Self::with(
            Box::new(Keychain(entry)),
            Box::new(TelegramApi(
                reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(15))
                    .build()?,
            )),
        )
    }
    fn with(vault: Box<dyn Vault>, api: Box<dyn BotApi>) -> Result<Self> {
        let saved = vault
            .load()?
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|_| anyhow::anyhow!("Invalid saved Telegram connection"))?;
        Ok(Self {
            vault,
            api,
            saved,
            pending: None,
            deliveries: VecDeque::new(),
        })
    }
    pub fn state(&self, now: u64) -> Value {
        let pending = self.pending.as_ref();
        json!({"connected":self.saved.is_some(),
            "bot":self.saved.as_ref().map(|s|s.username.as_str()),
            "chat":self.saved.as_ref().map(|s|s.chat_label.as_str()),
            "pending":pending.map(|p|json!({"bot":p.username,"start_url":format!("https://t.me/{}?start=mpac_{}",p.username,p.challenge),"command":format!("/start mpac_{}",p.challenge),"expires_at":p.started+300,"expired":now>=p.started+300,"candidate":p.candidate.as_ref().filter(|_|now<p.started+300).map(|(id,label)|json!({"id":id,"label":label}))}))})
    }
    pub fn renew(&mut self, now: u64) -> Result<Value> {
        let pending = self
            .pending
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Connect a bot first"))?;
        pending.challenge = mpac_mobile_link::secret()[..32].to_owned();
        pending.started = now;
        pending.candidate = None;
        Ok(self.state(now))
    }
    pub fn begin(&mut self, token: &str, now: u64) -> Result<Value> {
        ensure!(valid_token(token), "Invalid bot token format");
        let me = self.api.call(token, "getMe", json!({}))?;
        let bot_id = me["id"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid bot identity"))?;
        let username = me["username"]
            .as_str()
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or_else(|| anyhow::anyhow!("Invalid bot username"))?
            .to_string();
        ensure!(me["is_bot"] == true, "This token does not identify a bot");
        let webhook = self.api.call(token, "getWebhookInfo", json!({}))?;
        ensure!(
            webhook["url"].as_str() == Some(""),
            "This bot has a webhook. Use a dedicated bot; MPAC will not remove its webhook."
        );
        self.pending = Some(Pending {
            token: token.to_owned(),
            bot_id,
            username,
            challenge: mpac_mobile_link::secret()[..32].to_owned(),
            started: now,
            offset: 0,
            candidate: None,
        });
        Ok(self.state(now))
    }
    pub fn check(&mut self, now: u64) -> Result<Value> {
        let p = self
            .pending
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Connect a bot first"))?;
        ensure!(
            now < p.started + 300,
            "Bot connection expired. Connect the bot again."
        );
        let updates = self.api.call(
            &p.token,
            "getUpdates",
            json!({"offset":p.offset,"timeout":0,"limit":100,"allowed_updates":["message"]}),
        )?;
        let updates = updates
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Unexpected Telegram updates"))?;
        for update in updates {
            if let Some(id) = update["update_id"].as_i64() {
                p.offset = p.offset.max(id.saturating_add(1));
            }
            if let Some(candidate) = candidate(&update["message"], &p.challenge, p.started, now) {
                p.candidate = Some(candidate);
            }
        }
        Ok(self.state(now))
    }
    pub fn confirm(&mut self, chat_id: i64, now: u64) -> Result<Value> {
        let p = self
            .pending
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No pending bot connection"))?;
        ensure!(
            now < p.started + 300,
            "Bot connection expired. Connect the bot again."
        );
        let (id, label) = p
            .candidate
            .as_ref()
            .filter(|(id, _)| *id == chat_id)
            .ok_or_else(|| anyhow::anyhow!("Verify your private bot chat first"))?;
        let saved = Saved {
            token: p.token.clone(),
            bot_id: p.bot_id,
            username: p.username.clone(),
            chat_id: *id,
            chat_label: label.clone(),
        };
        self.vault.save(&serde_json::to_string(&saved)?)?;
        self.saved = Some(saved);
        self.pending = None;
        Ok(self.state(now))
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn disconnect(&mut self) -> Result<()> {
        self.vault.delete()?;
        self.saved = None;
        self.pending = None;
        Ok(())
    }
    pub fn send(
        &mut self,
        request_id: &str,
        make_link: impl FnOnce() -> Result<String>,
        pairing: bool,
        computer: &str,
    ) -> Result<Value> {
        ensure!(
            mpac_mobile_link::valid_secret(request_id),
            "Invalid delivery ID"
        );
        if let Some((_, sent)) = self.deliveries.iter().find(|(id, _)| id == request_id) {
            ensure!(
                *sent,
                "Delivery was not confirmed. Check Telegram before creating a new send request."
            );
            return Ok(json!({"sent":true}));
        }
        let saved = self
            .saved
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Connect your private Telegram chat first"))?;
        let link = make_link()?;
        ensure!(
            link.len() < 3500 && link.starts_with("https://"),
            "Use a public HTTPS opening page"
        );
        self.deliveries.push_back((request_id.to_owned(), false));
        if self.deliveries.len() > 100 {
            self.deliveries.pop_front();
        }
        let text = if pairing {
            "Connect this device to MPAC. This invitation expires in 3 minutes and requires approval on your computer."
        } else {
            "Open MPAC on this device. An existing connection and network access to your computer are required."
        };
        // Carry the complete invitation as a URL button. Plain-text auto-link
        // detection must not decide which part of its nested fragment opens.
        self.api.call(
            &saved.token,
            "sendMessage",
            json!({
                "chat_id": saved.chat_id,
                "text": format!("MPAC · {}\n{text}", computer.chars().filter(|c| !c.is_control()).take(80).collect::<String>()),
                "reply_markup": {"inline_keyboard": [[{
                    "text": if pairing { "Connect this device" } else { "Open MPAC" },
                    "url": link
                }]]},
                "link_preview_options": {"is_disabled": true}
            }),
        )?;
        self.deliveries.back_mut().unwrap().1 = true;
        Ok(json!({"sent":true}))
    }
}
fn valid_token(value: &str) -> bool {
    let Some((id, key)) = value.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.len() <= 20
        && id.bytes().all(|b| b.is_ascii_digit())
        && (20..=200).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn candidate(message: &Value, challenge: &str, started: u64, now: u64) -> Option<(i64, String)> {
    let id = message["chat"]["id"].as_i64()?;
    if id <= 0
        || message["chat"]["type"] != "private"
        || message["from"]["id"].as_i64() != Some(id)
        || message["from"]["is_bot"] != false
        || message["text"].as_str() != Some(format!("/start mpac_{challenge}").as_str())
        || !(started..=now.saturating_add(30)).contains(&message["date"].as_u64()?)
    {
        return None;
    }
    let label = message["from"]["username"]
        .as_str()
        .or_else(|| message["from"]["first_name"].as_str())
        .unwrap_or("Private chat");
    Some((
        id,
        format!("{} ({id})", label.chars().take(100).collect::<String>()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Memory(Arc<Mutex<Option<String>>>);
    impl Vault for Memory {
        fn load(&self) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &str) -> Result<()> {
            *self.0.lock().unwrap() = Some(s.into());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }
    struct Mock(Arc<Mutex<Vec<(String, Value)>>>);
    impl BotApi for Mock {
        fn call(&self, _: &str, m: &str, b: Value) -> Result<Value> {
            self.0.lock().unwrap().push((m.into(), b));
            Ok(match m {
                "getMe" => json!({"id":1,"username":"mpac_test_bot","is_bot":true}),
                "getWebhookInfo" => json!({"url":""}),
                "getUpdates" => json!([]),
                _ => json!({"message_id":1}),
            })
        }
    }
    fn open_link() -> Result<String> {
        Ok("https://mpac.works/open/".into())
    }
    fn message() -> Value {
        json!({"chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"username":"owner"},"text":"/start mpac_test","date":100})
    }
    #[test]
    fn binding_rejects_groups_forwarded_identity_stale_and_wrong_challenge() {
        let m = message();
        assert!(candidate(&m, "test", 100, 105).is_some());
        for bad in [
            json!({"chat":{"id":42,"type":"group"}}),
            json!({"from":{"id":7,"is_bot":false}}),
            json!({"date":99}),
            json!({"text":"/start mpac_wrong"}),
        ] {
            let mut m = message();
            for (k, v) in bad.as_object().unwrap() {
                m[k] = v.clone()
            }
            assert!(candidate(&m, "test", 100, 105).is_none());
        }
    }
    #[test]
    fn state_redacts_token_and_confirmation_is_explicit() {
        let calls = Arc::new(Mutex::new(vec![]));
        let memory = Memory::default();
        let saved = memory.0.clone();
        let mut tg = Telegram::with(Box::new(memory), Box::new(Mock(calls.clone()))).unwrap();
        let token = "123456:abcdefghijklmnopqrstuvxyz";
        let state = tg.begin(token, 100).unwrap();
        assert!(!state.to_string().contains(token));
        assert!(saved.lock().unwrap().is_none());
        assert!(tg.confirm(42, 101).is_err());
        assert!(
            tg.send(&mpac_mobile_link::secret(), open_link, false, "QA Mac")
                .is_err()
        );
        tg.pending.as_mut().unwrap().candidate = Some((42, "owner".into()));
        tg.confirm(42, 101).unwrap();
        assert!(saved.lock().unwrap().is_some());
        let id = mpac_mobile_link::secret();
        tg.send(&id, open_link, false, "QA Mac").unwrap();
        tg.send(&id, open_link, false, "QA Mac").unwrap();
        assert_eq!(
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, _)| m == "sendMessage")
                .count(),
            1
        );
        {
            let calls = calls.lock().unwrap();
            let body = &calls.last().unwrap().1;
            assert_eq!(body["chat_id"], 42);
            assert_eq!(
                body["reply_markup"]["inline_keyboard"][0][0]["text"],
                "Open MPAC"
            );
            assert_eq!(
                body["reply_markup"]["inline_keyboard"][0][0]["url"],
                open_link().unwrap()
            );
        }
        tg.begin(token, 102).unwrap();
        tg.cancel();
        assert!(tg.state(103)["connected"].as_bool().unwrap());
        tg.disconnect().unwrap();
        assert!(saved.lock().unwrap().is_none());
    }
    #[test]
    fn pairing_delivery_has_computer_specific_offer_and_duplicates_preserve_invitation() {
        let calls = Arc::new(Mutex::new(vec![]));
        let mut tg =
            Telegram::with(Box::<Memory>::default(), Box::new(Mock(calls.clone()))).unwrap();
        let mut pairing = mpac_mobile_link::pairing::Pairing::default();
        let mut generated = 0;
        let mut invitation = String::new();
        let id = mpac_mobile_link::secret();
        assert!(
            tg.send(
                &id,
                || panic!("unbound bot must not create an invitation"),
                true,
                "QA Mac"
            )
            .is_err()
        );
        tg.begin("123456:abcdefghijklmnopqrstuvxyz", 100).unwrap();
        tg.pending.as_mut().unwrap().candidate = Some((42, "owner".into()));
        tg.confirm(42, 101).unwrap();
        for _ in 0..2 {
            tg.send(
                &id,
                || {
                    generated += 1;
                    invitation = pairing.invite(102);
                    let offer = mpac_mobile_link::Offer {
                        endpoint: "https://192.168.1.16:4330/".into(),
                        certificate: vec![1, 2, 3],
                        invite: invitation.clone(),
                    };
                    mpac_mobile_link::sharing::link("https://mpac.works/open/", &offer.link()?)
                },
                true,
                "QA Mac\n",
            )
            .unwrap();
        }
        assert_eq!(
            generated, 1,
            "duplicate delivery must preserve the sent invitation"
        );
        let calls = calls.lock().unwrap();
        let messages: Vec<_> = calls
            .iter()
            .filter(|(method, _)| method == "sendMessage")
            .collect();
        assert_eq!(messages.len(), 1);
        let body = &messages[0].1;
        assert_eq!(body["chat_id"], 42);
        let text = body["text"].as_str().unwrap();
        assert!(text.starts_with("MPAC · QA Mac\nConnect this device"));
        assert!(text.contains("expires in 3 minutes"));
        assert!(!text.contains("https://"));
        let button = &body["reply_markup"]["inline_keyboard"][0][0];
        assert_eq!(button["text"], "Connect this device");
        let link = button["url"].as_str().unwrap();
        let offer = mpac_mobile_link::Offer::parse_input(link).unwrap();
        assert_eq!(offer.endpoint, "https://192.168.1.16:4330/");
        assert_eq!(offer.invite, invitation);
        assert_eq!(offer.certificate, vec![1, 2, 3]);
        assert_eq!(body["link_preview_options"]["is_disabled"], true);
        assert!(
            pairing
                .request(&invitation, &mpac_mobile_link::secret(), "QA Phone", 103)
                .is_ok()
        );
    }
    struct Offline(Arc<Mutex<usize>>);
    impl BotApi for Offline {
        fn call(&self, _: &str, method: &str, _: Value) -> Result<Value> {
            assert_eq!(method, "sendMessage");
            *self.0.lock().unwrap() += 1;
            bail!("Delivery not confirmed")
        }
    }
    #[test]
    fn uncertain_delivery_is_not_retried_and_saved_owner_survives_reload() {
        let memory = Memory::default();
        memory
            .save(
                &serde_json::to_string(&Saved {
                    token: "123456:abcdefghijklmnopqrstuvxyz".into(),
                    bot_id: 1,
                    username: "bot".into(),
                    chat_id: 42,
                    chat_label: "owner".into(),
                })
                .unwrap(),
            )
            .unwrap();
        let calls = Arc::new(Mutex::new(0));
        let mut tg = Telegram::with(Box::new(memory), Box::new(Offline(calls.clone()))).unwrap();
        assert_eq!(tg.state(100)["chat"], "owner");
        let id = mpac_mobile_link::secret();
        assert!(tg.send(&id, open_link, false, "QA Mac").is_err());
        assert!(tg.send(&id, open_link, false, "QA Mac").is_err());
        assert_eq!(*calls.lock().unwrap(), 1);
    }
    struct Webhook;
    impl BotApi for Webhook {
        fn call(&self, _: &str, method: &str, _: Value) -> Result<Value> {
            Ok(match method {
                "getMe" => json!({"id":1,"username":"bot","is_bot":true}),
                "getWebhookInfo" => json!({"url":"https://existing.example/bot"}),
                _ => panic!("must not mutate another service's bot"),
            })
        }
    }
    #[test]
    fn active_webhook_and_expired_challenges_never_replace_owner() {
        let mut tg = Telegram::with(Box::<Memory>::default(), Box::new(Webhook)).unwrap();
        assert!(tg.begin("123456:abcdefghijklmnopqrstuvxyz", 100).is_err());
        assert!(tg.pending.is_none());
        let mut tg = Telegram::with(
            Box::<Memory>::default(),
            Box::new(Mock(Arc::new(Mutex::new(vec![])))),
        )
        .unwrap();
        tg.begin("123456:abcdefghijklmnopqrstuvxyz", 100).unwrap();
        tg.pending.as_mut().unwrap().candidate = Some((42, "owner".into()));
        assert!(tg.confirm(42, 400).is_err());
        assert!(tg.check(400).is_err());
        assert_eq!(tg.state(400)["pending"]["expired"], true);
        assert_eq!(tg.state(400)["connected"], false);
    }
    #[test]
    fn renewal_changes_code_without_saving_or_sending_and_rejects_old_starts() {
        let memory = Memory::default();
        let stored = memory.0.clone();
        let calls = Arc::new(Mutex::new(vec![]));
        let mut tg = Telegram::with(Box::new(memory), Box::new(Mock(calls.clone()))).unwrap();
        let first = tg.begin("123456:abcdefghijklmnopqrstuvxyz", 100).unwrap();
        tg.pending.as_mut().unwrap().candidate = Some((42, "owner".into()));
        assert_eq!(tg.state(401)["pending"]["expired"], true);
        let fresh = tg.renew(401).unwrap();
        assert_ne!(first["pending"]["command"], fresh["pending"]["command"]);
        assert_eq!(fresh["pending"]["expired"], false);
        assert_eq!(fresh["pending"]["candidate"], Value::Null);
        assert!(tg.confirm(42, 402).is_err());
        assert!(stored.lock().unwrap().is_none());
        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "renewal must not send a message or revalidate the token"
        );
        let p = tg.pending.as_ref().unwrap();
        let mut m = message();
        m["date"] = json!(402);
        m["text"] = first["pending"]["command"].clone();
        assert!(candidate(&m, &p.challenge, p.started, 402).is_none());
        m["text"] = fresh["pending"]["command"].clone();
        assert!(candidate(&m, &p.challenge, p.started, 402).is_some());
        m["text"] = json!("/start");
        assert!(candidate(&m, &p.challenge, p.started, 402).is_none());
    }
}

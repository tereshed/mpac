use super::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Provider {
    #[default]
    OpenAI,
    OpenRouter,
    Groq,
    Custom,
}

impl Provider {
    const ALL: [Self; 4] = [Self::OpenAI, Self::OpenRouter, Self::Groq, Self::Custom];

    fn defaults(self) -> (&'static str, &'static str) {
        match self {
            Self::OpenAI => ("https://api.openai.com/v1", "gpt-transcribe"),
            Self::OpenRouter => ("https://openrouter.ai/api/v1", "openai/whisper-large-v3"),
            Self::Groq => ("https://api.groq.com/openai/v1", "whisper-large-v3-turbo"),
            Self::Custom => ("", ""),
        }
    }

    fn from_url(url: &Url) -> Self {
        Self::ALL
            .into_iter()
            .find(|provider| url.as_str().trim_end_matches('/') == provider.defaults().0)
            .unwrap_or(Self::Custom)
    }

    fn url(self, value: &str) -> Result<Url> {
        let url = base_url(value)?;
        if self != Self::Custom && url.as_str().trim_end_matches('/') != self.defaults().0 {
            return Err("voice_url");
        }
        Ok(url)
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Profile {
    base_url: String,
    pub(super) model: String,
    pub(super) api_key: String,
}

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Settings {
    enabled: bool,
    provider: Provider,
    profiles: BTreeMap<Provider, Profile>,
    revision: String,
}

// The previous release stored a single profile under the same keychain entry.
#[derive(Deserialize)]
struct LegacySettings {
    enabled: bool,
    revision: String,
    #[serde(flatten)]
    profile: Profile,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredSettings {
    Current(Settings),
    Legacy(LegacySettings),
}

impl Settings {
    pub(super) fn from_stored(value: &str) -> Result<Self> {
        match serde_json::from_str(value).map_err(|_| "voice_storage")? {
            StoredSettings::Current(settings) => Ok(settings),
            StoredSettings::Legacy(legacy) => {
                let provider = Provider::from_url(&base_url(&legacy.profile.base_url)?);
                Ok(Self {
                    enabled: legacy.enabled,
                    provider,
                    profiles: BTreeMap::from([(provider, legacy.profile)]),
                    revision: legacy.revision,
                })
            }
        }
    }

    pub(super) fn public(&self) -> Value {
        let providers: Vec<Value> = Provider::ALL
            .into_iter()
            .map(|id| {
                let (url, model) = id.defaults();
                let saved = self.profiles.get(&id);
                json!({
                    "id": id,
                    "base_url": saved.map_or(url, |p| p.base_url.as_str()),
                    "model": saved.map_or(model, |p| p.model.as_str()),
                    "has_key": saved.is_some_and(|p| !p.api_key.is_empty()),
                })
            })
            .collect();
        let active = self.profiles.get(&self.provider);
        let (default_url, default_model) = self.provider.defaults();
        json!({
            "enabled": self.enabled,
            "provider": self.provider,
            "providers": providers,
            "base_url": active.map_or(default_url, |p| p.base_url.as_str()),
            "model": active.map_or(default_model, |p| p.model.as_str()),
            "has_key": active.is_some_and(|p| !p.api_key.is_empty()),
            "revision": self.revision,
            "capture": if cfg!(target_vendor = "apple") { "native" } else { "web" },
        })
    }

    pub(super) fn ready(&self, revision: &str) -> Result<(&Profile, Url)> {
        let profile = self.profiles.get(&self.provider).ok_or("voice_disabled")?;
        if !self.enabled || profile.api_key.is_empty() {
            return Err("voice_disabled");
        }
        if self.revision != revision {
            return Err("voice_settings_changed");
        }
        let mut url = self.provider.url(&profile.base_url)?;
        url.path_segments_mut()
            .map_err(|_| "voice_url")?
            .pop_if_empty()
            .extend(["audio", "transcriptions"]);
        Ok((profile, url))
    }

    pub(super) fn update(mut self, update: Update) -> Result<Self> {
        let provider = match update.provider {
            Some(provider) => provider,
            None => Provider::from_url(&base_url(&update.base_url)?),
        };
        if update.remove_key {
            if let Some(profile) = self.profiles.get_mut(&provider) {
                profile.api_key.clear();
            }
            if self.provider == provider {
                self.enabled = false;
            }
            self.revision = Uuid::new_v4().to_string();
            return Ok(self);
        }

        let url = provider.url(&update.base_url)?;
        let model = update.model.trim();
        if model.is_empty() || model.len() > 128 || model.chars().any(char::is_control) {
            return Err("voice_model");
        }
        let api_key = if let Some(key) = update.api_key {
            let key = key.trim();
            if key.is_empty()
                || key.len() > 4096
                || !key.bytes().all(|byte| byte.is_ascii_graphic())
            {
                return Err("voice_key");
            }
            key.to_owned()
        } else if let Some(profile) = self.profiles.get(&provider) {
            if !profile.api_key.is_empty() && base_url(&profile.base_url)?.origin() != url.origin()
            {
                return Err("voice_new_key");
            }
            profile.api_key.clone()
        } else {
            String::new()
        };
        if update.enabled && api_key.is_empty() {
            return Err("voice_key");
        }
        self.profiles.insert(
            provider,
            Profile {
                base_url: url.as_str().trim_end_matches('/').into(),
                model: model.into(),
                api_key,
            },
        );
        self.provider = provider;
        self.enabled = update.enabled;
        self.revision = Uuid::new_v4().to_string();
        Ok(self)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    #[serde(default)]
    provider: Option<Provider>,
    enabled: bool,
    base_url: String,
    model: String,
    api_key: Option<String>,
    #[serde(default)]
    remove_key: bool,
}

fn base_url(value: &str) -> Result<Url> {
    if value.len() > 2048 {
        return Err("voice_url");
    }
    let url = Url::parse(value.trim()).map_err(|_| "voice_url")?;
    if url.scheme() != "https"
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url
            .path()
            .trim_end_matches('/')
            .ends_with("/audio/transcriptions")
    {
        return Err("voice_url");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(provider: Provider, key: Option<&str>) -> Update {
        let (url, model) = provider.defaults();
        Update {
            provider: Some(provider),
            enabled: true,
            base_url: url.into(),
            model: model.into(),
            api_key: key.map(str::to_owned),
            remove_key: false,
        }
    }

    #[test]
    fn migrates_single_profile_and_restores_provider_specific_keys() {
        let legacy = r#"{"enabled":true,"base_url":"https://api.openai.com/v1/","model":"my-model","api_key":"old-secret","revision":"old"}"#;
        let settings = Settings::from_stored(legacy).unwrap();
        assert_eq!(settings.ready("old").unwrap().0.model, "my-model");
        let settings = settings
            .update(change(Provider::OpenRouter, Some("router-secret")))
            .unwrap();
        let stored = serde_json::to_string(&settings).unwrap();
        let settings = Settings::from_stored(&stored).unwrap();
        let (router, url) = settings.ready(&settings.revision).unwrap();
        assert_eq!(
            url.as_str(),
            "https://openrouter.ai/api/v1/audio/transcriptions"
        );
        assert_eq!(router.api_key, "router-secret");
        assert!(!settings.public().to_string().contains("secret"));
        assert!(matches!(
            settings.ready("old"),
            Err("voice_settings_changed")
        ));
        let mut openai = change(Provider::OpenAI, None);
        openai.model = settings.profiles[&Provider::OpenAI].model.clone();
        let settings = settings.update(openai).unwrap();
        let (profile, _) = settings.ready(&settings.revision).unwrap();
        assert_eq!(
            (profile.api_key.as_str(), profile.model.as_str()),
            ("old-secret", "my-model")
        );
    }

    #[test]
    fn never_reuses_a_key_for_another_provider_or_custom_origin() {
        let make = || {
            Settings::default()
                .update(change(Provider::OpenAI, Some("openai-key")))
                .unwrap()
        };
        assert!(matches!(
            make().update(change(Provider::OpenRouter, None)),
            Err("voice_key")
        ));
        let mut forged = change(Provider::OpenAI, None);
        forged.base_url = Provider::OpenRouter.defaults().0.into();
        assert!(matches!(make().update(forged), Err("voice_url")));
        let old = r#"{"enabled":true,"base_url":"https://speech.example/v1","model":"custom","api_key":"custom-key","revision":"old"}"#;
        let mut custom = change(Provider::Custom, None);
        custom.base_url = "https://new.example/v1".into();
        custom.model = "custom".into();
        assert!(matches!(
            Settings::from_stored(old).unwrap().update(custom),
            Err("voice_new_key")
        ));
        assert!(base_url("http://speech.example/v1").is_err());
        assert!(base_url("https://user:password@speech.example/v1").is_err());
        assert!(base_url("https://speech.example/v1?token=secret").is_err());
    }

    #[test]
    fn removing_an_inactive_key_keeps_the_active_provider() {
        let settings = Settings::default()
            .update(change(Provider::OpenAI, Some("openai-key")))
            .unwrap()
            .update(change(Provider::Groq, Some("groq-key")))
            .unwrap();
        let mut remove = change(Provider::OpenAI, None);
        remove.remove_key = true;
        let settings = settings.update(remove).unwrap();
        assert!(settings.profiles[&Provider::OpenAI].api_key.is_empty());
        let (profile, url) = settings.ready(&settings.revision).unwrap();
        assert_eq!(profile.api_key, "groq-key");
        assert_eq!(
            url.as_str(),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
    }
}

//! Phone credentials and pinned HTTPS transport. No daemon credential is sent
//! over the network or returned to a renderer. POSTs are never retried.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use url::Url;
pub mod pairing;
pub mod sharing;

pub const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn secret_hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
pub fn equal(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |x, (a, b)| x | (a ^ b)) == 0
}
pub fn valid_secret(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// LAN, loopback and explicitly paired private overlays (Tailscale/CGNAT).
/// Public addresses and DNS names remain outside the preview's trust policy.
pub fn private_host_address(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
}

// Deliberately no Debug: this includes a one-time invitation.
pub struct Offer {
    pub endpoint: String,
    pub certificate: Vec<u8>,
    pub invite: String,
}
pub fn endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("Invalid computer address")?;
    ensure!(
        url.scheme() == "https"
            && url.port().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Use the exact HTTPS address from the computer"
    );
    let ip: std::net::Ipv4Addr = url
        .host_str()
        .context("Missing computer address")?
        .parse()
        .context("This local preview requires an IPv4 address")?;
    ensure!(
        private_host_address(ip),
        "Use a local or private overlay network address"
    );
    Ok(url)
}
impl Offer {
    /// Pasted HTTPS opening links are unwrapped locally, never fetched.
    pub fn parse_input(value: &str) -> Result<Self> {
        ensure!(value.len() <= 16384, "Pairing link is too large");
        let mut url = Url::parse(value)?;
        if url.scheme() != "https" {
            return Self::parse(value);
        }
        let fragment = url.fragment().unwrap_or_default().to_string();
        url.set_fragment(None);
        crate::sharing::page(url.as_str())?;
        let fields: Vec<_> = url::form_urlencoded::parse(fragment.as_bytes()).collect();
        ensure!(
            fields.len() == 1 && fields[0].0 == "link",
            "Invalid opening link"
        );
        Self::parse(&fields[0].1)
    }
    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 8192, "Pairing link is too large");
        let url = Url::parse(value).context("Invalid pairing link")?;
        ensure!(
            url.scheme() == "mpac"
                && url.host_str() == Some("pair")
                && matches!(url.path(), "" | "/")
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none(),
            "Invalid pairing link"
        );
        let pairs: Vec<_> = url.query_pairs().collect();
        ensure!(
            pairs.len() == 2
                && pairs.iter().filter(|(k, _)| k == "v").count() == 1
                && pairs.iter().filter(|(k, _)| k == "endpoint").count() == 1,
            "Invalid pairing link fields"
        );
        ensure!(
            pairs.iter().any(|(k, v)| k == "v" && v == "1"),
            "Unsupported pairing link version"
        );
        let address = pairs
            .iter()
            .find(|(k, _)| k == "endpoint")
            .unwrap()
            .1
            .to_string();
        endpoint(&address)?;
        let fields: Vec<_> =
            url::form_urlencoded::parse(url.fragment().unwrap_or("").as_bytes()).collect();
        ensure!(
            fields.len() == 2
                && fields.iter().filter(|(k, _)| k == "ca").count() == 1
                && fields.iter().filter(|(k, _)| k == "invite").count() == 1,
            "Invalid pairing link trust data"
        );
        let certificate = URL_SAFE_NO_PAD
            .decode(fields.iter().find(|(k, _)| k == "ca").unwrap().1.as_bytes())
            .context("Invalid computer certificate")?;
        ensure!(
            !certificate.is_empty() && certificate.len() <= 4096,
            "Invalid computer certificate"
        );
        let invite = fields
            .iter()
            .find(|(k, _)| k == "invite")
            .unwrap()
            .1
            .to_string();
        ensure!(valid_secret(&invite), "Invalid pairing invitation");
        Ok(Self {
            endpoint: address,
            certificate,
            invite,
        })
    }
    pub fn link(&self) -> Result<String> {
        endpoint(&self.endpoint)?;
        let mut url = Url::parse("mpac://pair")?;
        url.query_pairs_mut()
            .append_pair("v", "1")
            .append_pair("endpoint", &self.endpoint);
        let fragment = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("ca", &URL_SAFE_NO_PAD.encode(&self.certificate))
            .append_pair("invite", &self.invite)
            .finish();
        url.set_fragment(Some(&fragment));
        Ok(url.into())
    }
}

// Serialized only into the OS credential store, never IPC/debug/logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub endpoint: String,
    pub certificate: Vec<u8>,
    pub device_key: String,
    pub device_id: String,
    pub computer_name: String,
}
pub struct RemoteClient {
    profile: Profile,
    client: reqwest::blocking::Client,
}
#[derive(Serialize, Deserialize)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}
impl RemoteClient {
    pub fn new(profile: Profile) -> Result<Self> {
        endpoint(&profile.endpoint)?;
        ensure!(
            valid_secret(&profile.device_key),
            "Invalid device credential"
        );
        let cert = reqwest::Certificate::from_der(&profile.certificate)
            .context("Invalid computer certificate")?;
        let client = reqwest::blocking::Client::builder()
            .tls_certs_only([cert])
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .context("Could not create the encrypted connection")?;
        Ok(Self { profile, client })
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn request(&self, method: &str, path: &str, body: &[u8]) -> Result<Reply> {
        ensure!(
            matches!(method, "GET" | "POST") && safe_api_path(path),
            "Unsupported mobile request"
        );
        self.send(method, path, body)
    }
    pub fn pairing_request(&self, path: &str, body: &[u8]) -> Result<Reply> {
        ensure!(
            matches!(path, "/pair/request" | "/pair/status"),
            "Invalid pairing request"
        );
        self.send("POST", path, body)
    }
    fn send(&self, method: &str, path: &str, body: &[u8]) -> Result<Reply> {
        use std::io::Read;
        ensure!(body.len() <= 16 * 1024 * 1024, "Request is too large");
        let address = format!("{}{}", self.profile.endpoint.trim_end_matches('/'), path);
        // Build exactly once; neither middleware nor a reconnect retries input.
        let response = self
            .client
            .request(reqwest::Method::from_bytes(method.as_bytes())?, address)
            .bearer_auth(&self.profile.device_key)
            .header("content-type", "application/json")
            .body(body.to_vec())
            .send()
            .map_err(|_| {
                anyhow::anyhow!(
                    "Computer unavailable or certificate changed. No command was retried."
                )
            })?;
        let status = response.status().as_u16();
        ensure!(!(300..400).contains(&status), "Computer redirect rejected");
        let mut bytes = Vec::new();
        response
            .take(MAX_RESPONSE as u64 + 1)
            .read_to_end(&mut bytes)
            .context("Connection interrupted; check the result before sending again")?;
        ensure!(
            bytes.len() <= MAX_RESPONSE,
            "Computer response is too large"
        );
        let body = String::from_utf8(bytes).context("Invalid computer response")?;
        Ok(Reply { status, body })
    }
}
pub fn safe_api_path(path: &str) -> bool {
    path.starts_with("/api/")
        && path.len() <= 16 * 1024
        && !path.contains(['\\', '\r', '\n', '#'])
        && path
            .split('?')
            .next()
            .is_some_and(|p| !p.contains('%') && p.split('/').all(|s| s != "." && s != ".."))
        && !path.starts_with("/api/native/")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_overlay_addresses_have_exact_cgnat_boundaries() {
        for ip in ["100.64.0.0", "100.100.1.2", "100.127.255.255"] {
            assert!(endpoint(&format!("https://{ip}:4330/")).is_ok());
        }
        for ip in ["100.63.255.255", "100.128.0.0", "135.181.6.114", "0.0.0.0"] {
            assert!(endpoint(&format!("https://{ip}:4330/")).is_err());
        }
    }
    #[test]
    fn offers_pin_exact_endpoint_and_reject_ambiguous_fields() {
        let offer = Offer {
            endpoint: "https://192.168.1.2:4320/".into(),
            certificate: vec![1, 2, 3],
            invite: secret(),
        };
        let link = offer.link().unwrap();
        let parsed = Offer::parse(&link).unwrap();
        assert_eq!(parsed.endpoint, offer.endpoint);
        assert_eq!(parsed.certificate, offer.certificate);
        assert!(Offer::parse(&link.replace("v=1", "v=1&v=1")).is_err());
        assert!(Offer::parse(&link.replace("mpac://pair", "https://pair")).is_err());
        assert!(Offer::parse(&(link + "&invite=" + &secret())).is_err());
        for bad in [
            "http://192.168.1.2:4320/",
            "https://user@192.168.1.2:4320/",
            "https://192.168.1.2:4320/path",
            "https://example.com:4320/",
            "https://8.8.8.8:4320/",
        ] {
            assert!(endpoint(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn request_paths_cannot_escape_to_other_services() {
        assert!(safe_api_path("/api/projects/abc/file?path=docs%2Fhello.md"));
        for path in [
            "https://elsewhere/api/state",
            "//elsewhere/api/state",
            "/api/../admin",
            "/api/%2e%2e/admin",
            "/api/native/terminal",
            "/api/state\r\nX: y",
        ] {
            assert!(!safe_api_path(path), "{path}");
        }
    }
    #[test]
    fn credentials_are_random_bounded_and_hashed() {
        let a = secret();
        let b = secret();
        assert_ne!(a, b);
        assert!(valid_secret(&a));
        assert!(equal(&secret_hash(&a), &secret_hash(&a)));
        assert!(!equal(&a, &b));
        assert!(!valid_secret("short"));
    }
}

use anyhow::{Result, ensure};
use url::Url;

pub fn page(value: &str) -> Result<Url> {
    let mut url = Url::parse(value)?;
    let host = url.host_str().unwrap_or_default();
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && matches!(url.path(), "/" | "/open" | "/open/")
            && host.contains('.')
            && !host.ends_with(".local")
            && !host.ends_with('.')
            && host.parse::<std::net::IpAddr>().is_err(),
        "Use a public HTTPS opening page without query or fragment"
    );
    url.set_path("/open/");
    Ok(url)
}

pub fn link(page_url: &str, invitation: &str) -> Result<String> {
    crate::Offer::parse(invitation)?;
    let mut url = page(page_url)?;
    url.set_fragment(Some(
        &url::form_urlencoded::Serializer::new(String::new())
            .append_pair("link", invitation)
            .finish(),
    ));
    Ok(url.into())
}
pub fn telegram(link: &str) -> Result<String> {
    let url = Url::parse(link)?;
    ensure!(url.scheme() == "https", "Share the HTTPS opening link");
    let mut share = Url::parse("https://t.me/share/url")?;
    share
        .query_pairs_mut()
        .append_pair("url", link)
        .append_pair("text", "Open MPAC on your device");
    Ok(share.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secrets_stay_in_fragment_and_telegram_encodes_the_entire_link() {
        let invite = crate::Offer {
            endpoint: "https://192.168.1.4:4330/".into(),
            certificate: vec![1, 2, 3],
            invite: "a".repeat(64),
        }
        .link()
        .unwrap();
        let wrapped = link("https://links.example.com/open/", &invite).unwrap();
        let url = Url::parse(&wrapped).unwrap();
        assert!(url.query().is_none());
        assert_eq!(url.path(), "/open/");
        assert_eq!(
            url::form_urlencoded::parse(url.fragment().unwrap().as_bytes())
                .next()
                .unwrap()
                .1,
            invite
        );
        let parsed = crate::Offer::parse_input(&wrapped).unwrap();
        assert_eq!(parsed.invite, "a".repeat(64));
        assert!(
            crate::Offer::parse_input("https://links.example.com/open/#link=mpac%3A%2F%2Fexec")
                .is_err()
        );
        let share = telegram(&wrapped).unwrap();
        assert!(Url::parse(&share).unwrap().fragment().is_none());
        assert_eq!(
            Url::parse(&share)
                .unwrap()
                .query_pairs()
                .find(|(k, _)| k == "url")
                .unwrap()
                .1,
            wrapped
        );
    }
    #[test]
    fn untrusted_page_configuration_is_rejected() {
        for url in [
            "http://example.com/",
            "https://127.0.0.1/",
            "https://localhost/",
            "https://example.com/?key=secret",
            "https://user@example.com/",
            "https://example.com/other",
        ] {
            assert!(page(url).is_err());
        }
    }
}

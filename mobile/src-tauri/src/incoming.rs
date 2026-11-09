use mpac_mobile_link::Offer;
use serde::Serialize;
use std::time::{Duration, Instant};

// One-time invitations deliberately have no Debug implementation.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IncomingLink {
    Pair { link: String, endpoint: String },
    Open,
    Invalid,
}

impl IncomingLink {
    fn parse(value: &str) -> Self {
        if matches!(value, "mpac://open" | "mpac://open/") {
            return Self::Open;
        }
        if value.len() > 16384 {
            return Self::Invalid;
        }
        if let Ok(url) = tauri::Url::parse(value)
            && url.scheme() == "https"
        {
            if url.host_str() != Some("mpac.works")
                || !matches!(url.path(), "/open" | "/open/")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
            {
                return Self::Invalid;
            }
            if url.fragment().is_none() {
                return Self::Open;
            }
        }
        match Offer::parse_input(value) {
            Ok(offer) => Self::Pair {
                link: value.into(),
                endpoint: offer.endpoint,
            },
            Err(_) => Self::Invalid,
        }
    }
}

#[derive(Serialize)]
pub struct Delivery {
    pub sequence: u64,
    #[serde(flatten)]
    pub link: IncomingLink,
}

#[derive(Default)]
pub struct Inbox {
    pub hydrated: bool,
    pub sequence: u64,
    pending: Option<IncomingLink>,
    pending_sequence: u64,
    last: Option<(String, Instant)>,
}
impl Inbox {
    pub fn receive(&mut self, value: &str) {
        if self
            .last
            .as_ref()
            .is_some_and(|(last, at)| last == value && at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        let item = IncomingLink::parse(value);
        self.sequence += 1;
        // A shortcut or malformed URL must not discard a pending invitation.
        if matches!(item, IncomingLink::Pair { .. }) || self.pending.is_none() {
            self.pending = Some(item);
            self.pending_sequence = self.sequence;
        }
        self.last = Some((value.chars().take(8193).collect(), Instant::now()));
    }
    /// Keep the invitation until the renderer confirms that it committed it.
    /// A suspended/reloaded webview must not consume a link without showing it.
    pub fn peek(&self) -> Option<Delivery> {
        Some(Delivery {
            sequence: self.pending_sequence,
            link: self.pending.as_ref()?.clone(),
        })
    }
    pub fn acknowledge(&mut self, sequence: u64) {
        if self.pending_sequence == sequence {
            self.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_waits_for_explicit_ui_action_and_duplicate_delivery_is_consumed_once() {
        let link = Offer {
            endpoint: "https://192.168.1.4:4330/".into(),
            certificate: vec![1, 2, 3],
            invite: "a".repeat(64),
        }
        .link()
        .unwrap();
        let mut inbox = Inbox::default();
        inbox.receive(&link);
        inbox.receive(&link);
        assert_eq!(inbox.sequence, 1);
        inbox.receive("mpac://open");
        assert!(
            matches!(inbox.peek().map(|d| d.link), Some(IncomingLink::Pair { endpoint, .. }) if endpoint == "https://192.168.1.4:4330/")
        );
        let delivery = inbox.peek().unwrap();
        assert!(inbox.peek().is_some());
        inbox.acknowledge(delivery.sequence);
        assert!(inbox.peek().is_none());
    }
    #[test]
    fn arbitrary_schemes_commands_and_invalid_trust_data_are_never_dispatched() {
        for link in [
            "https://elsewhere/",
            "mpac://exec?cmd=rm",
            "mpac://open?command=run",
            "mpac://pair?v=1&endpoint=http://localhost/",
        ] {
            let mut inbox = Inbox::default();
            inbox.receive(link);
            assert!(matches!(
                inbox.peek().map(|d| d.link),
                Some(IncomingLink::Invalid)
            ));
        }
    }

    #[test]
    fn https_invitation_is_unwrapped_without_fetching_or_pairing() {
        let raw = Offer {
            endpoint: "https://192.168.1.4:4330/".into(),
            certificate: vec![1, 2, 3],
            invite: "a".repeat(64),
        }
        .link()
        .unwrap();
        let wrapped = mpac_mobile_link::sharing::link("https://mpac.works/open/", &raw).unwrap();
        let mut inbox = Inbox::default();
        inbox.receive(&wrapped);
        assert!(
            matches!(inbox.peek().unwrap().link, IncomingLink::Pair { link, endpoint } if link == wrapped && endpoint == "https://192.168.1.4:4330/")
        );
        let old = inbox.peek().unwrap().sequence;
        inbox.receive(&raw);
        inbox.acknowledge(old);
        assert!(
            inbox.peek().is_some(),
            "stale renderer acknowledgement must not remove a newer link"
        );
        for invalid in [
            wrapped.replace("mpac.works", "other.example"),
            wrapped.replace("/open/", "/elsewhere/"),
            wrapped.replace("https://", "https://user@"),
            format!("{wrapped}&extra=1"),
        ] {
            assert!(matches!(
                IncomingLink::parse(&invalid),
                IncomingLink::Invalid
            ));
        }
        assert!(matches!(
            IncomingLink::parse("https://mpac.works/open/"),
            IncomingLink::Open
        ));
    }
    #[test]
    fn ignored_events_and_old_acknowledgements_cannot_consume_a_pending_invitation() {
        let raw = Offer {
            endpoint: "https://192.168.1.4:4330/".into(),
            certificate: vec![1, 2, 3],
            invite: "a".repeat(64),
        }
        .link()
        .unwrap();
        let mut inbox = Inbox::default();
        inbox.receive(&raw);
        let sequence = inbox.peek().unwrap().sequence;
        inbox.receive("mpac://open");
        inbox.receive("mpac://exec?cmd=run");
        assert_eq!(inbox.peek().unwrap().sequence, sequence);
        inbox.acknowledge(sequence + 1);
        assert!(inbox.peek().is_some());
        inbox.acknowledge(sequence);
        assert!(inbox.peek().is_none());
        // A plugin echo immediately after renderer acknowledgement is not a new invitation.
        inbox.receive("mpac://exec?cmd=run");
        assert!(inbox.peek().is_none());
    }

    #[test]
    fn https_routes_and_trust_data_are_strictly_validated() {
        for value in [
            "https://mpac.works/open/?redirect=mpac://open",
            "https://mpac.works.evil.example/open/",
            "https://mpac.works:444/open/",
            "https://mpac.works/open/#link=mpac%3A%2F%2Fexec",
            "https://mpac.works/open/#link=mpac%3A%2F%2Fopen",
            "https://mpac.works/",
        ] {
            assert!(matches!(IncomingLink::parse(value), IncomingLink::Invalid));
        }
        assert!(matches!(
            IncomingLink::parse(&"x".repeat(16385)),
            IncomingLink::Invalid
        ));
    }
}

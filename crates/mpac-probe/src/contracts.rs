use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Protocol,
    InteractivePty,
}

impl ExecutionMode {
    pub fn accepts_terminal_input(self) -> bool {
        self == Self::InteractivePty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingRoute {
    Subscription,
    CodingPlan,
    Metered,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtraUsageControl {
    ProviderEnforcedDisabled,
    Enabled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingEvidence {
    pub route: BillingRoute,
    pub extra_usage: ExtraUsageControl,
    /// This must come from a real enforcement mechanism. A login response or
    /// user selection alone is not such a mechanism.
    pub evidence_expires_at: Option<u64>,
}

impl BillingEvidence {
    pub fn subscription_only_assured(&self, now: u64) -> bool {
        matches!(
            self.route,
            BillingRoute::Subscription | BillingRoute::CodingPlan
        ) && self.extra_usage == ExtraUsageControl::ProviderEnforcedDisabled
            && self
                .evidence_expires_at
                .is_some_and(|expires| expires > now)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkout {
    pub repository_id: String,
    pub canonical_path: String,
    pub revision: String,
    pub writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub host_id: String,
    pub execution_generation: u64,
    pub mode: ExecutionMode,
    pub profile_revision: String,
    pub checkouts: Vec<Checkout>,
}

impl WorkspaceManifest {
    /// All-or-nothing routing proof. Filesystem canonicalization and permission
    /// enforcement belong to the future daemon; this is the frozen-ID contract.
    pub fn accepts_review(&self, host: &str, targets: &[Checkout]) -> bool {
        let mapping: BTreeMap<_, _> = self
            .checkouts
            .iter()
            .map(|c| (&c.repository_id, c))
            .collect();
        let unique: BTreeSet<_> = self.checkouts.iter().map(|c| &c.repository_id).collect();
        host == self.host_id
            && unique.len() == self.checkouts.len()
            && !targets.is_empty()
            && targets.iter().all(|target| {
                mapping.get(&target.repository_id).is_some_and(|actual| {
                    actual.writable
                        && actual.canonical_path == target.canonical_path
                        && actual.revision == target.revision
                })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBinding {
    pub authority_epoch: u64,
    pub execution_generation: u64,
    pub request_id: String,
    pub action_revision: String,
}

/// Restore starts a fresh authority namespace, never a replay of old grants.
pub fn approval_matches(
    current: &RequestBinding,
    incoming: &RequestBinding,
    pending: bool,
    grant_valid: bool,
) -> bool {
    pending && grant_valid && current == incoming
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_alone_never_proves_no_additional_spend() {
        let mut e = BillingEvidence {
            route: BillingRoute::Subscription,
            extra_usage: ExtraUsageControl::Unknown,
            evidence_expires_at: Some(100),
        };
        assert!(!e.subscription_only_assured(1));
        e.extra_usage = ExtraUsageControl::Enabled;
        assert!(!e.subscription_only_assured(1));
        e.extra_usage = ExtraUsageControl::ProviderEnforcedDisabled;
        assert!(e.subscription_only_assured(1));
        assert!(!e.subscription_only_assured(100));
        e.route = BillingRoute::Metered;
        assert!(!e.subscription_only_assured(1));
    }

    #[test]
    fn stale_or_missing_enforcement_expires() {
        let e = BillingEvidence {
            route: BillingRoute::CodingPlan,
            extra_usage: ExtraUsageControl::ProviderEnforcedDisabled,
            evidence_expires_at: None,
        };
        assert!(!e.subscription_only_assured(0));
    }

    #[test]
    fn protocol_stdin_is_not_a_terminal() {
        assert!(!ExecutionMode::Protocol.accepts_terminal_input());
        assert!(ExecutionMode::InteractivePty.accepts_terminal_input());
    }

    fn checkout(id: &str, path: &str) -> Checkout {
        Checkout {
            repository_id: id.into(),
            canonical_path: path.into(),
            revision: "snapshot-1".into(),
            writable: true,
        }
    }

    #[test]
    fn multi_repo_dispatch_requires_exact_checkout_mapping() {
        let backend = checkout("backend", "/work/backend-A");
        let frontend = checkout("frontend", "/work/frontend-A");
        let manifest = WorkspaceManifest {
            host_id: "host-A".into(),
            execution_generation: 1,
            mode: ExecutionMode::Protocol,
            profile_revision: "profile-1".into(),
            checkouts: vec![backend.clone(), frontend.clone()],
        };
        assert!(manifest.accepts_review("host-A", &[backend.clone(), frontend.clone()]));
        assert!(!manifest.accepts_review("host-B", std::slice::from_ref(&backend)));
        assert!(!manifest.accepts_review("host-A", &[checkout("frontend", "/work/frontend-B")]));
        assert!(!manifest.accepts_review("host-A", &[checkout("missing", "/work/backend-A")]));
        let mut changed = frontend.clone();
        changed.revision = "snapshot-2".into();
        assert!(!manifest.accepts_review("host-A", &[changed]));
        let mut readonly = manifest.clone();
        readonly.checkouts[1].writable = false;
        assert!(!readonly.accepts_review("host-A", &[backend, frontend]));
    }

    #[test]
    fn duplicate_repository_identity_is_rejected() {
        let c = checkout("repo", "/work/a");
        let m = WorkspaceManifest {
            host_id: "h".into(),
            execution_generation: 1,
            mode: ExecutionMode::Protocol,
            profile_revision: "p".into(),
            checkouts: vec![c.clone(), c.clone()],
        };
        assert!(!m.accepts_review("h", &[c]));
    }

    #[test]
    fn restore_generation_reuse_and_revoke_cannot_approve_old_action() {
        let current = RequestBinding {
            authority_epoch: 2,
            execution_generation: 3,
            request_id: "1".into(),
            action_revision: "hash-A".into(),
        };
        assert!(approval_matches(&current, &current, true, true));
        for stale in [
            RequestBinding {
                authority_epoch: 1,
                ..current.clone()
            },
            RequestBinding {
                execution_generation: 2,
                ..current.clone()
            },
            RequestBinding {
                action_revision: "hash-B".into(),
                ..current.clone()
            },
        ] {
            assert!(!approval_matches(&current, &stale, true, true));
        }
        assert!(!approval_matches(&current, &current, false, true));
        assert!(!approval_matches(&current, &current, true, false));
    }
}

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use camellia_nexus_licensing::SecretValue;

pub(crate) const AUTHORIZATION_SESSION_CAPACITY: usize = 8;
pub(crate) const AUTHORIZATION_SESSION_TTL: Duration = Duration::from_secs(3 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingAuthorizationError {
    Missing,
    Expired,
    CompletionInProgress,
}

pub(crate) struct PendingAuthorizationCredentials {
    pub(crate) pkce_verifier: SecretValue,
    pub(crate) redirect_uri: String,
}

struct PendingAuthorization {
    credentials: PendingAuthorizationCredentials,
    expires_at: Instant,
    completion_in_progress: bool,
    ordinal: u64,
}

pub(crate) struct PendingAuthorizationStore {
    entries: BTreeMap<String, PendingAuthorization>,
    capacity: usize,
    ttl: Duration,
    next_ordinal: u64,
}

impl Default for PendingAuthorizationStore {
    fn default() -> Self {
        Self::new(AUTHORIZATION_SESSION_CAPACITY, AUTHORIZATION_SESSION_TTL)
    }
}

impl PendingAuthorizationStore {
    fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            entries: BTreeMap::new(),
            capacity: capacity.max(1),
            ttl,
            next_ordinal: 0,
        }
    }

    /// Inserts a new authorization session and returns states evicted by TTL/capacity policy.
    pub(crate) fn insert(
        &mut self,
        state: String,
        pkce_verifier: SecretValue,
        redirect_uri: String,
    ) -> Vec<String> {
        let now = Instant::now();
        let mut removed = self.purge_expired(now);
        self.next_ordinal = self.next_ordinal.wrapping_add(1);
        if self.entries.contains_key(&state) {
            self.entries.remove(&state);
            removed.push(state.clone());
        }
        while self.entries.len() >= self.capacity {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.ordinal)
                .map(|(state, _)| state.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
            removed.push(oldest);
        }
        self.entries.insert(
            state,
            PendingAuthorization {
                credentials: PendingAuthorizationCredentials {
                    pkce_verifier,
                    redirect_uri,
                },
                expires_at: now + self.ttl,
                completion_in_progress: false,
                ordinal: self.next_ordinal,
            },
        );
        removed
    }

    pub(crate) fn begin_completion(
        &mut self,
        state: &str,
    ) -> Result<PendingAuthorizationCredentials, PendingAuthorizationError> {
        let now = Instant::now();
        let Some(entry) = self.entries.get(state) else {
            return Err(PendingAuthorizationError::Missing);
        };
        if entry.expires_at <= now {
            self.entries.remove(state);
            return Err(PendingAuthorizationError::Expired);
        }
        let entry = self
            .entries
            .get_mut(state)
            .expect("entry was checked above");
        if entry.completion_in_progress {
            return Err(PendingAuthorizationError::CompletionInProgress);
        }
        entry.completion_in_progress = true;
        Ok(PendingAuthorizationCredentials {
            pkce_verifier: entry.credentials.pkce_verifier.clone(),
            redirect_uri: entry.credentials.redirect_uri.clone(),
        })
    }

    pub(crate) fn finish_completion(&mut self, state: &str, succeeded: bool) {
        if succeeded {
            self.entries.remove(state);
            return;
        }
        let now = Instant::now();
        if self
            .entries
            .get(state)
            .is_some_and(|entry| entry.expires_at <= now)
        {
            self.entries.remove(state);
        } else if let Some(entry) = self.entries.get_mut(state) {
            entry.completion_in_progress = false;
        }
    }

    pub(crate) fn cancel(&mut self, state: &str) -> bool {
        self.entries.remove(state).is_some()
    }

    pub(crate) fn clear(&mut self) -> Vec<String> {
        std::mem::take(&mut self.entries).into_keys().collect()
    }

    pub(crate) fn contains_active(&mut self, state: &str) -> bool {
        let now = Instant::now();
        if self
            .entries
            .get(state)
            .is_some_and(|entry| entry.expires_at > now)
        {
            true
        } else {
            self.entries.remove(state);
            false
        }
    }

    fn purge_expired(&mut self, now: Instant) -> Vec<String> {
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(state, _)| state.clone())
            .collect::<Vec<_>>();
        for state in &expired {
            self.entries.remove(state);
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verifier(value: &str) -> SecretValue {
        SecretValue(value.repeat(32))
    }

    #[test]
    fn completion_is_single_flight_and_can_retry_after_failure() {
        let mut store = PendingAuthorizationStore::new(2, Duration::from_secs(60));
        store.insert(
            "state".into(),
            verifier("v"),
            "http://127.0.0.1/callback".into(),
        );

        assert!(store.begin_completion("state").is_ok());
        assert!(matches!(
            store.begin_completion("state"),
            Err(PendingAuthorizationError::CompletionInProgress)
        ));
        store.finish_completion("state", false);
        assert!(store.begin_completion("state").is_ok());
        store.finish_completion("state", true);
        assert!(matches!(
            store.begin_completion("state"),
            Err(PendingAuthorizationError::Missing)
        ));
    }

    #[test]
    fn capacity_evicts_oldest_session() {
        let mut store = PendingAuthorizationStore::new(2, Duration::from_secs(60));
        store.insert("first".into(), verifier("a"), "one".into());
        store.insert("second".into(), verifier("b"), "two".into());
        let removed = store.insert("third".into(), verifier("c"), "three".into());

        assert_eq!(removed, vec!["first"]);
        assert!(!store.contains_active("first"));
        assert!(store.contains_active("second"));
        assert!(store.contains_active("third"));
        assert!(store.cancel("second"));
        assert!(!store.contains_active("second"));
    }

    #[test]
    fn expired_session_cannot_complete() {
        let mut store = PendingAuthorizationStore::new(1, Duration::ZERO);
        store.insert("state".into(), verifier("v"), "redirect".into());
        assert!(matches!(
            store.begin_completion("state"),
            Err(PendingAuthorizationError::Expired)
        ));
    }

    #[test]
    fn cancellation_during_completion_cannot_resurrect_the_session() {
        let mut store = PendingAuthorizationStore::new(1, Duration::from_secs(60));
        store.insert("state".into(), verifier("v"), "redirect".into());
        assert!(store.begin_completion("state").is_ok());
        assert!(store.cancel("state"));

        store.finish_completion("state", false);
        assert!(matches!(
            store.begin_completion("state"),
            Err(PendingAuthorizationError::Missing)
        ));
    }

    #[test]
    fn clearing_store_invalidates_every_pending_session() {
        let mut store = PendingAuthorizationStore::new(2, Duration::from_secs(60));
        store.insert("first".into(), verifier("a"), "one".into());
        store.insert("second".into(), verifier("b"), "two".into());

        assert_eq!(store.clear(), vec!["first", "second"]);
        assert!(!store.contains_active("first"));
        assert!(!store.contains_active("second"));
    }
}

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use serde::{Deserialize, Serialize};

use crate::{DynSecureStore, LicensingError, Result, SecretKey, get_json, put_json};

const DEFAULT_ROLLBACK_TOLERANCE_SECONDS: i64 = 5 * 60;
const REBOOT_ELAPSED_TOLERANCE_SECONDS: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedTimeRecord {
    /// Durable trusted-time high-water mark. It starts from an authenticated server response and
    /// may advance at an accepted reboot checkpoint.
    pub server_unix: i64,
    /// Highest time obtained directly from an authenticated online response. Keeping this
    /// separate allows a fast local clock to be corrected by the server later.
    pub last_online_server_unix: i64,
    pub observed_local_unix: i64,
    /// Process-independent monotonic time elapsed since the operating system booted.
    pub observed_boot_elapsed_seconds: i64,
    /// Operating-system boot/session identifier used to distinguish a reboot
    /// from a process restart even when uptime happens to be greater.
    pub boot_identifier: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedTimeObservation {
    pub unix: i64,
    pub rollback_detected: bool,
}

#[derive(Clone)]
pub struct TrustedTime {
    store: DynSecureStore,
    rollback_tolerance_seconds: i64,
    runtime: Arc<Mutex<RuntimeTimeState>>,
    uptime: Arc<dyn UptimeSource>,
}

enum RuntimeTimeState {
    Uninitialized,
    Missing,
    Anchored(RuntimeTimeAnchor),
}

struct RuntimeTimeAnchor {
    trusted_unix: i64,
    local_unix: i64,
    boot_elapsed_seconds: i64,
    observed_at: Instant,
    last_online_server_unix: i64,
}

trait UptimeSource: Send + Sync {
    fn elapsed_seconds(&self) -> Result<i64>;
    fn boot_identifier(&self) -> Result<String>;
}

struct SystemUptimeSource;

impl UptimeSource for SystemUptimeSource {
    fn elapsed_seconds(&self) -> Result<i64> {
        system_uptime_seconds()
    }

    fn boot_identifier(&self) -> Result<String> {
        system_boot_identifier()
    }
}

impl TrustedTime {
    pub fn new(store: DynSecureStore) -> Self {
        Self {
            store,
            rollback_tolerance_seconds: DEFAULT_ROLLBACK_TOLERANCE_SECONDS,
            runtime: Arc::new(Mutex::new(RuntimeTimeState::Uninitialized)),
            uptime: Arc::new(SystemUptimeSource),
        }
    }

    pub fn with_rollback_tolerance(mut self, seconds: i64) -> Self {
        self.rollback_tolerance_seconds = seconds.max(0);
        self
    }

    pub fn record_server_time(&self, server_unix: i64, local_unix: i64) -> Result<()> {
        if server_unix <= 0 || local_unix <= 0 {
            return Err(LicensingError::InvalidServerResponse);
        }
        let boot_elapsed_seconds = self.uptime.elapsed_seconds()?;
        let boot_identifier = self.uptime.boot_identifier()?;
        let _ = self.observe(local_unix)?;
        let previous_online = match &*self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RuntimeTimeState::Anchored(anchor) => Some(anchor.last_online_server_unix),
            RuntimeTimeState::Uninitialized | RuntimeTimeState::Missing => None,
        };
        if previous_online.is_some_and(|previous| {
            server_unix.saturating_add(self.rollback_tolerance_seconds) < previous
        }) {
            return Err(LicensingError::InvalidServerResponse);
        }
        // Tolerance permits harmless response reordering; it must never lower
        // the authenticated high-water mark or repeated small skews could
        // ratchet trusted time backwards.
        let server_unix = previous_online.map_or(server_unix, |previous| previous.max(server_unix));
        put_json(
            self.store.as_ref(),
            SecretKey::TrustedTime,
            &TrustedTimeRecord {
                server_unix,
                last_online_server_unix: server_unix,
                observed_local_unix: local_unix,
                observed_boot_elapsed_seconds: boot_elapsed_seconds,
                boot_identifier,
            },
        )?;
        *self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            RuntimeTimeState::Anchored(RuntimeTimeAnchor {
                trusted_unix: server_unix,
                local_unix,
                boot_elapsed_seconds,
                observed_at: Instant::now(),
                last_online_server_unix: server_unix,
            });
        Ok(())
    }

    pub fn observe(&self, local_unix: i64) -> Result<Option<TrustedTimeObservation>> {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*runtime, RuntimeTimeState::Uninitialized) {
            let Some(record): Option<TrustedTimeRecord> =
                get_json(self.store.as_ref(), SecretKey::TrustedTime)?
            else {
                *runtime = RuntimeTimeState::Missing;
                return Ok(None);
            };
            validate_record(&record)?;
            let boot_elapsed_seconds = self.uptime.elapsed_seconds()?;
            let boot_identifier = self.uptime.boot_identifier()?;
            let (elapsed, rollback_detected) = elapsed_since_persisted_anchor(
                &record,
                local_unix,
                boot_elapsed_seconds,
                &boot_identifier,
                self.rollback_tolerance_seconds,
            );
            let trusted_unix = record.server_unix.saturating_add(elapsed);
            if !rollback_detected && boot_identifier != record.boot_identifier {
                // Checkpoint an accepted boot transition exactly once. Without this durable
                // transition, repeatedly starting from new boot identifiers with a frozen wall
                // clock could reuse the same persisted anchor indefinitely.
                put_json(
                    self.store.as_ref(),
                    SecretKey::TrustedTime,
                    &TrustedTimeRecord {
                        server_unix: trusted_unix,
                        last_online_server_unix: record.last_online_server_unix,
                        observed_local_unix: local_unix,
                        observed_boot_elapsed_seconds: boot_elapsed_seconds,
                        boot_identifier: boot_identifier.clone(),
                    },
                )?;
            }
            *runtime = RuntimeTimeState::Anchored(RuntimeTimeAnchor {
                trusted_unix,
                local_unix,
                boot_elapsed_seconds,
                observed_at: Instant::now(),
                last_online_server_unix: record.last_online_server_unix,
            });
            return Ok(Some(TrustedTimeObservation {
                unix: trusted_unix,
                rollback_detected,
            }));
        }
        let RuntimeTimeState::Anchored(anchor) = &mut *runtime else {
            return Ok(None);
        };
        let now = Instant::now();
        let monotonic_elapsed = duration_seconds(anchor.observed_at.elapsed());
        let current_boot_elapsed_seconds = self.uptime.elapsed_seconds()?;
        let boot_elapsed = current_boot_elapsed_seconds
            .saturating_sub(anchor.boot_elapsed_seconds)
            .max(0);
        let monotonic_elapsed = monotonic_elapsed.max(boot_elapsed);
        let expected_local_unix = anchor.local_unix.saturating_add(monotonic_elapsed);
        let rollback_detected =
            local_unix.saturating_add(self.rollback_tolerance_seconds) < expected_local_unix;
        let local_elapsed = local_unix.saturating_sub(anchor.local_unix).max(0);
        let elapsed = monotonic_elapsed.max(local_elapsed);
        let trusted_unix = anchor.trusted_unix.saturating_add(elapsed);
        if local_elapsed > monotonic_elapsed {
            anchor.trusted_unix = trusted_unix;
            anchor.local_unix = local_unix;
            anchor.boot_elapsed_seconds = current_boot_elapsed_seconds;
            anchor.observed_at = now;
        }
        Ok(Some(TrustedTimeObservation {
            unix: trusted_unix,
            rollback_detected,
        }))
    }

    /// Durably applies a lower bound learned from an authenticated denial. `trusted_lower_bound`
    /// may include a later local observation, while `online_lower_bound` must contain only the
    /// time floor proven by the server decision. This distinction preserves online correction of
    /// a fast local clock.
    pub(crate) fn checkpoint_authenticated_lower_bound(
        &self,
        trusted_lower_bound: i64,
        online_lower_bound: i64,
        local_unix: i64,
    ) -> Result<i64> {
        if trusted_lower_bound <= 0 || online_lower_bound < 0 || local_unix <= 0 {
            return Err(LicensingError::InvalidServerResponse);
        }
        let current = self
            .observe(local_unix)?
            .map(|observation| observation.unix)
            .unwrap_or(trusted_lower_bound);
        let trusted_unix = current.max(trusted_lower_bound);
        let boot_elapsed_seconds = self.uptime.elapsed_seconds()?;
        let boot_identifier = self.uptime.boot_identifier()?;
        let previous_online = match &*self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            RuntimeTimeState::Anchored(anchor) => anchor.last_online_server_unix,
            RuntimeTimeState::Uninitialized | RuntimeTimeState::Missing => 0,
        };
        let last_online_server_unix = previous_online.max(online_lower_bound);
        put_json(
            self.store.as_ref(),
            SecretKey::TrustedTime,
            &TrustedTimeRecord {
                server_unix: trusted_unix,
                last_online_server_unix,
                observed_local_unix: local_unix,
                observed_boot_elapsed_seconds: boot_elapsed_seconds,
                boot_identifier,
            },
        )?;
        *self
            .runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            RuntimeTimeState::Anchored(RuntimeTimeAnchor {
                trusted_unix,
                local_unix,
                boot_elapsed_seconds,
                observed_at: Instant::now(),
                last_online_server_unix,
            });
        Ok(trusted_unix)
    }
}

fn validate_record(record: &TrustedTimeRecord) -> Result<()> {
    if record.server_unix <= 0
        || record.last_online_server_unix < 0
        || record.last_online_server_unix > record.server_unix
        || record.observed_local_unix <= 0
        || record.observed_boot_elapsed_seconds < 0
        || record.boot_identifier.is_empty()
        || record.boot_identifier.len() > 128
    {
        return Err(LicensingError::SecureStoreCorrupt);
    }
    Ok(())
}

fn elapsed_since_persisted_anchor(
    record: &TrustedTimeRecord,
    local_unix: i64,
    boot_elapsed_seconds: i64,
    boot_identifier: &str,
    rollback_tolerance_seconds: i64,
) -> (i64, bool) {
    let local_elapsed = local_unix.saturating_sub(record.observed_local_unix).max(0);
    if boot_identifier == record.boot_identifier
        && boot_elapsed_seconds >= record.observed_boot_elapsed_seconds
    {
        let boot_elapsed = boot_elapsed_seconds - record.observed_boot_elapsed_seconds;
        let expected_local = record.observed_local_unix.saturating_add(boot_elapsed);
        return (
            local_elapsed.max(boot_elapsed),
            local_unix.saturating_add(rollback_tolerance_seconds) < expected_local,
        );
    }

    // The boot identifier changed (or monotonic time reset), so a reboot
    // occurred after the checkpoint. Wall time must cover at least the time
    // already spent in the current boot; otherwise it was moved backwards or
    // frozen.
    let rollback_detected = local_unix.saturating_add(rollback_tolerance_seconds)
        < record.observed_local_unix
        || local_elapsed.saturating_add(REBOOT_ELAPSED_TOLERANCE_SECONDS) < boot_elapsed_seconds;
    (
        if rollback_detected {
            local_elapsed
        } else {
            local_elapsed.max(boot_elapsed_seconds)
        },
        rollback_detected,
    )
}

#[cfg(target_os = "linux")]
fn system_uptime_seconds() -> Result<i64> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `value` points to writable timespec storage and CLOCK_BOOTTIME
    // is a platform-defined monotonic clock identifier.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, value.as_mut_ptr()) } != 0 {
        return Err(LicensingError::SecureStoreBackend);
    }
    // SAFETY: clock_gettime returned success and initialized `value`.
    let value = unsafe { value.assume_init() };
    i64::try_from(i128::from(value.tv_sec)).map_err(|_| LicensingError::SecureStoreBackend)
}

#[cfg(target_os = "linux")]
fn system_boot_identifier() -> Result<String> {
    let identifier = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| LicensingError::SecureStoreBackend)?;
    normalize_boot_identifier(&identifier)
}

#[cfg(target_os = "macos")]
fn system_uptime_seconds() -> Result<i64> {
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    #[link(name = "System")]
    unsafe extern "C" {
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
    // SAFETY: `timebase` points to writable storage owned by this function.
    if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
        return Err(LicensingError::SecureStoreBackend);
    }
    // SAFETY: mach_continuous_time has no preconditions and returns ticks from
    // a boot-scoped monotonic clock which continues while the system sleeps.
    let ticks = unsafe { mach_continuous_time() };
    let nanoseconds =
        u128::from(ticks).saturating_mul(u128::from(timebase.numer)) / u128::from(timebase.denom);
    i64::try_from(nanoseconds / 1_000_000_000).map_err(|_| LicensingError::SecureStoreBackend)
}

#[cfg(target_os = "macos")]
fn system_boot_identifier() -> Result<String> {
    let name = c"kern.bootsessionuuid";
    let mut length = 0_usize;
    // SAFETY: the first sysctlbyname call only writes the required size to
    // `length`; the name is a static NUL-terminated C string.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || length == 0
        || length > 129
    {
        return Err(LicensingError::SecureStoreBackend);
    }
    let mut value = vec![0_u8; length];
    // SAFETY: `value` owns `length` writable bytes and the remaining pointers
    // satisfy sysctlbyname's read-only query contract.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(LicensingError::SecureStoreBackend);
    }
    value.truncate(length);
    if value.last() == Some(&0) {
        value.pop();
    }
    let identifier = String::from_utf8(value).map_err(|_| LicensingError::SecureStoreBackend)?;
    normalize_boot_identifier(&identifier)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn system_uptime_seconds() -> Result<i64> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: `value` points to writable timespec storage and CLOCK_MONOTONIC
    // is a platform-defined monotonic clock identifier.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) } != 0 {
        return Err(LicensingError::SecureStoreBackend);
    }
    // SAFETY: clock_gettime returned success and initialized `value`.
    let value = unsafe { value.assume_init() };
    i64::try_from(i128::from(value.tv_sec)).map_err(|_| LicensingError::SecureStoreBackend)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn system_boot_identifier() -> Result<String> {
    Err(LicensingError::SecureStoreBackend)
}

#[cfg(windows)]
fn system_uptime_seconds() -> Result<i64> {
    // SAFETY: GetTickCount64 has no preconditions and cannot fail.
    let milliseconds = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    i64::try_from(milliseconds / 1_000).map_err(|_| LicensingError::SecureStoreBackend)
}

#[cfg(windows)]
fn system_boot_identifier() -> Result<String> {
    use windows::Wdk::System::SystemInformation::{
        NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS,
    };

    #[repr(C)]
    #[derive(Default)]
    struct SystemBootEnvironmentInformation {
        boot_identifier: windows::core::GUID,
        firmware_type: i32,
        boot_flags: u64,
    }

    // SystemBootEnvironmentInformation is the documented information class 90.
    const BOOT_ENVIRONMENT_INFORMATION: SYSTEM_INFORMATION_CLASS = SYSTEM_INFORMATION_CLASS(90);
    let mut information = SystemBootEnvironmentInformation::default();
    let mut returned_length = 0_u32;
    // SAFETY: `information` is correctly laid out writable storage for the
    // requested class and its exact byte length is supplied to ntdll.
    let status = unsafe {
        NtQuerySystemInformation(
            BOOT_ENVIRONMENT_INFORMATION,
            std::ptr::from_mut(&mut information).cast(),
            u32::try_from(std::mem::size_of::<SystemBootEnvironmentInformation>())
                .map_err(|_| LicensingError::SecureStoreBackend)?,
            &mut returned_length,
        )
    };
    if status.0 < 0
        || returned_length
            < u32::try_from(std::mem::size_of::<SystemBootEnvironmentInformation>())
                .map_err(|_| LicensingError::SecureStoreBackend)?
        || information.boot_identifier == windows::core::GUID::zeroed()
    {
        return Err(LicensingError::SecureStoreBackend);
    }
    normalize_boot_identifier(&format!("{:?}", information.boot_identifier))
}

#[cfg(not(any(unix, windows)))]
fn system_uptime_seconds() -> Result<i64> {
    Err(LicensingError::SecureStoreBackend)
}

#[cfg(not(any(unix, windows)))]
fn system_boot_identifier() -> Result<String> {
    Err(LicensingError::SecureStoreBackend)
}

fn normalize_boot_identifier(identifier: &str) -> Result<String> {
    let identifier = identifier.trim();
    if identifier.is_empty()
        || identifier.len() > 128
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(LicensingError::SecureStoreBackend);
    }
    Ok(identifier.to_ascii_lowercase())
}

fn duration_seconds(duration: std::time::Duration) -> i64 {
    duration.as_secs().min(i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, AtomicUsize, Ordering},
    };

    use super::*;
    use crate::{SecureStore, SecureStoreMode, SessionSecureStore};

    #[derive(Default)]
    struct CountingStore {
        inner: SessionSecureStore,
        reads: AtomicUsize,
        writes: AtomicUsize,
    }

    impl SecureStore for CountingStore {
        fn mode(&self) -> SecureStoreMode {
            self.inner.mode()
        }

        fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.inner.get_secret(key)
        }

        fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            self.inner.put_secret(key, value)
        }

        fn delete_secret(&self, key: SecretKey) -> Result<()> {
            self.inner.delete_secret(key)
        }
    }

    struct TestUptime {
        seconds: AtomicI64,
        boot_identifier: Mutex<String>,
    }

    impl TestUptime {
        fn new(seconds: i64) -> Self {
            Self {
                seconds: AtomicI64::new(seconds),
                boot_identifier: Mutex::new("boot-a".into()),
            }
        }

        fn set(&self, seconds: i64) {
            self.seconds.store(seconds, Ordering::Relaxed);
        }

        fn set_boot_identifier(&self, identifier: &str) {
            *self
                .boot_identifier
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = identifier.into();
        }
    }

    impl UptimeSource for TestUptime {
        fn elapsed_seconds(&self) -> Result<i64> {
            Ok(self.seconds.load(Ordering::Relaxed))
        }

        fn boot_identifier(&self) -> Result<String> {
            Ok(self
                .boot_identifier
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }
    }

    fn trusted_with_uptime(store: DynSecureStore, uptime: Arc<dyn UptimeSource>) -> TrustedTime {
        TrustedTime {
            store,
            rollback_tolerance_seconds: DEFAULT_ROLLBACK_TOLERANCE_SECONDS,
            runtime: Arc::new(Mutex::new(RuntimeTimeState::Uninitialized)),
            uptime,
        }
    }

    #[test]
    fn estimates_time_and_detects_significant_rollback() {
        let trusted =
            TrustedTime::new(Arc::new(SessionSecureStore::default())).with_rollback_tolerance(10);
        trusted.record_server_time(1_000, 2_000).expect("record");
        assert_eq!(
            trusted.observe(2_030).expect("observe"),
            Some(TrustedTimeObservation {
                unix: 1_030,
                rollback_detected: false,
            })
        );
        assert!(
            trusted
                .observe(1_980)
                .expect("observe")
                .unwrap()
                .rollback_detected
        );
    }

    #[test]
    fn secure_store_anchor_is_loaded_only_once_per_runtime() {
        let store = Arc::new(CountingStore::default());
        put_json(
            store.as_ref(),
            SecretKey::TrustedTime,
            &TrustedTimeRecord {
                server_unix: 1_000,
                last_online_server_unix: 1_000,
                observed_local_unix: 2_000,
                observed_boot_elapsed_seconds: system_uptime_seconds().expect("system uptime"),
                boot_identifier: system_boot_identifier().expect("boot identifier"),
            },
        )
        .expect("anchor");
        let trusted = TrustedTime::new(store.clone());

        assert!(trusted.observe(2_010).expect("first").is_some());
        assert!(trusted.observe(2_011).expect("second").is_some());
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn system_uptime_preserves_progress_across_frequent_frozen_clock_restarts() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        let trusted = trusted_with_uptime(store.clone(), uptime.clone());
        trusted.record_server_time(1_000, 2_000).expect("record");
        for elapsed in 1..=20 {
            uptime.set(500 + elapsed * 15);
            let restarted = trusted_with_uptime(store.clone(), uptime.clone());
            assert_eq!(
                restarted.observe(2_000).unwrap().unwrap().unix,
                1_000 + elapsed * 15
            );
        }

        uptime.set(815);
        assert!(
            trusted_with_uptime(store, uptime)
                .observe(2_000)
                .unwrap()
                .unwrap()
                .rollback_detected,
            "restarts cannot repeatedly reset the rollback tolerance window"
        );
    }

    #[test]
    fn offline_observation_does_not_periodically_rewrite_secure_storage() {
        let store = Arc::new(CountingStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        let trusted = trusted_with_uptime(store.clone(), uptime.clone());
        trusted.record_server_time(1_000, 2_000).expect("record");
        let writes_after_anchor = store.writes.load(Ordering::Relaxed);

        uptime.set(86_900);
        assert_eq!(trusted.observe(88_400).unwrap().unwrap().unix, 87_400);
        assert_eq!(store.writes.load(Ordering::Relaxed), writes_after_anchor);
    }

    #[test]
    fn frozen_wall_clock_after_reboot_requires_online_revalidation() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        let trusted = trusted_with_uptime(store.clone(), uptime.clone());
        trusted.record_server_time(1_000, 2_000).expect("record");

        uptime.set(30);
        uptime.set_boot_identifier("boot-b");
        let restarted = trusted_with_uptime(store, uptime);
        assert!(restarted.observe(2_000).unwrap().unwrap().rollback_detected);
    }

    #[test]
    fn reboot_identifier_prevents_greater_uptime_from_looking_like_a_process_restart() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(10));
        trusted_with_uptime(store.clone(), uptime.clone())
            .record_server_time(1_000, 2_000)
            .expect("record");

        uptime.set(20);
        uptime.set_boot_identifier("boot-b");
        let observation = trusted_with_uptime(store, uptime)
            .observe(2_000)
            .unwrap()
            .unwrap();
        assert!(observation.rollback_detected);
        assert_eq!(observation.unix, 1_000);
    }

    #[test]
    fn legitimate_offline_reboot_uses_consistent_wall_clock_progress() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        trusted_with_uptime(store.clone(), uptime.clone())
            .record_server_time(1_000, 2_000)
            .expect("record");

        uptime.set(30);
        uptime.set_boot_identifier("boot-b");
        let observation = trusted_with_uptime(store, uptime)
            .observe(3_000)
            .unwrap()
            .unwrap();
        assert!(!observation.rollback_detected);
        assert_eq!(observation.unix, 2_000);
    }

    #[test]
    fn accepted_boot_transitions_are_checkpointed_and_cannot_freeze_time() {
        let store = Arc::new(CountingStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        trusted_with_uptime(store.clone(), uptime.clone())
            .record_server_time(1_000, 2_000)
            .expect("record");
        let writes_after_online_anchor = store.writes.load(Ordering::Relaxed);

        // Even a sequence of unrealistically fast boots within the rounding tolerance advances
        // and checkpoints the trusted high-water mark instead of reusing the online anchor.
        for (index, boot) in ["boot-b", "boot-c", "boot-d"].into_iter().enumerate() {
            uptime.set(1);
            uptime.set_boot_identifier(boot);
            let observation = trusted_with_uptime(store.clone(), uptime.clone())
                .observe(2_000)
                .expect("observe")
                .expect("anchor");
            assert!(!observation.rollback_detected);
            assert_eq!(observation.unix, 1_001 + index as i64);
        }
        assert_eq!(
            store.writes.load(Ordering::Relaxed),
            writes_after_online_anchor + 3
        );

        uptime.set(30);
        uptime.set_boot_identifier("boot-e");
        assert!(
            trusted_with_uptime(store, uptime)
                .observe(2_000)
                .expect("observe")
                .expect("anchor")
                .rollback_detected,
            "a frozen wall clock cannot hide meaningful time spent in a later boot"
        );
    }

    #[test]
    fn offline_boot_checkpoint_does_not_block_authenticated_clock_correction() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        trusted_with_uptime(store.clone(), uptime.clone())
            .record_server_time(1_000, 2_000)
            .expect("record");

        uptime.set(30);
        uptime.set_boot_identifier("boot-b");
        let restarted = trusted_with_uptime(store, uptime);
        assert_eq!(restarted.observe(3_000).unwrap().unwrap().unix, 2_000);
        restarted
            .record_server_time(1_005, 3_001)
            .expect("server corrects a fast local clock after reboot");
        assert_eq!(restarted.observe(3_001).unwrap().unwrap().unix, 1_005);
    }

    #[test]
    fn rejects_online_time_rollback_but_allows_online_correction_of_a_fast_local_clock() {
        let store = Arc::new(SessionSecureStore::default());
        let trusted = TrustedTime::new(store).with_rollback_tolerance(10);
        trusted.record_server_time(1_000, 2_000).expect("record");
        assert!(matches!(
            trusted.record_server_time(980, 2_001),
            Err(LicensingError::InvalidServerResponse)
        ));

        assert!(trusted.observe(20_000).unwrap().unwrap().unix > 1_000);
        trusted
            .record_server_time(1_005, 20_000)
            .expect("authenticated server time corrects local clock skew");
        assert_eq!(trusted.observe(20_000).unwrap().unwrap().unix, 1_005);
    }

    #[test]
    fn tolerated_online_reordering_cannot_ratchet_the_high_water_mark_backwards() {
        let store = Arc::new(SessionSecureStore::default());
        let uptime = Arc::new(TestUptime::new(500));
        let trusted = trusted_with_uptime(store, uptime).with_rollback_tolerance(10);
        trusted.record_server_time(1_000, 2_000).expect("record");
        trusted
            .record_server_time(995, 2_001)
            .expect("minor reordering");
        trusted
            .record_server_time(991, 2_002)
            .expect("minor reordering cannot ratchet the comparison base");
        assert_eq!(trusted.observe(2_002).unwrap().unwrap().unix, 1_000);
        assert!(matches!(
            trusted.record_server_time(989, 2_003),
            Err(LicensingError::InvalidServerResponse)
        ));
    }
}

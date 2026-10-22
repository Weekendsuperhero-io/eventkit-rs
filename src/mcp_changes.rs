//! Change notifications for the calendar and reminder resources.
//!
//! EventKit posts `EKEventStoreChangedNotification` whenever anything in the
//! calendar database changes, from any process, without saying what. One
//! observer per process turns that into a count ([`signal`]). Each
//! subscription keeps a fingerprint of what it watches and, on a change,
//! re-reads it on the EventKit thread and sends only what differs:
//! `notifications/resources/updated` for each changed URI, and
//! `notifications/resources/list_changed` when the resource list did.
//!
//! - A 2025-11-25 connection hears every list change and the URIs it
//!   subscribed to (`resources/subscribe`), through its peer: [`Connection`],
//!   from the moment it has initialized.
//! - A 2026-07-28 client holds `subscriptions/listen` open, and [`listen`]
//!   watches what its filter accepted for as long as it does.
//!
//! Two things change what a resource reads with no store change: the day (a
//! window with no `from` starts at midnight) and access being granted (the
//! per-calendar resources appear). A watch also wakes at local midnight, and
//! [`note_access`] counts a change when a call has moved access.
//!
//! A watch never asks for access: the consent dialog belongs to a person's or
//! an agent's own call, not to a subscription. Without full access a watched
//! resource's fingerprint is the access status itself, so being granted access
//! is a change its subscriber hears about.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use chrono::{DateTime, Days, Local, TimeZone};
use objc2_foundation::{NSNotification, NSNotificationCenter};
use parking_lot::Mutex;
use rmcp::{
    Peer, RoleServer,
    model::{ResourceUpdatedNotificationParam, SubscriptionFilter},
    service::SubscriptionSink,
};
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::{eventkit_thread, resources};
use crate::{AuthorizationStatus, EventsManager, RemindersManager};

/// How long a change waits for the rest of its burst before anything is
/// re-read: an iCloud sync posts many in a row.
const SETTLE: Duration = Duration::from_millis(500);

// ── The signal ──────────────────────────────────────────────────────────

static SIGNAL: OnceLock<watch::Sender<u64>> = OnceLock::new();

/// Access to events and reminders as last looked at ([`note_access`]).
static ACCESS: Mutex<Option<(AuthorizationStatus, AuthorizationStatus)>> = Mutex::new(None);

/// The process's change count. The first call registers the one observer and
/// records access as it stands, for [`note_access`] to compare against.
fn signal() -> watch::Receiver<u64> {
    SIGNAL
        .get_or_init(|| {
            let (sender, _) = watch::channel(0);
            observe(sender.clone());
            *ACCESS.lock() = Some(access());
            sender
        })
        .subscribe()
}

fn bump(count: &watch::Sender<u64>) {
    count.send_modify(|count| *count = count.wrapping_add(1));
}

/// Observe `EKEventStoreChangedNotification` from any store (`object` nil:
/// the store cache replaces its store when access changes, and the new one
/// posts too). No queue, so the block runs on whichever thread EventKit posts
/// from, and it touches nothing but the count. Observed for the life of the
/// process: the token is never removed.
fn observe(count: watch::Sender<u64>) {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| bump(&count));
    // SAFETY: EventKit's exported notification name, and a block that only
    // counts, on whatever thread it is called.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(objc2_event_kit::EKEventStoreChangedNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token);
}

fn access() -> (AuthorizationStatus, AuthorizationStatus) {
    (
        EventsManager::authorization_status(),
        RemindersManager::authorization_status(),
    )
}

/// Count a change when access to events or reminders has moved since it was
/// last looked at: being granted access makes the per-calendar resources
/// listable and every resource readable, and no store notification says so.
/// Two status reads, after each call that may have asked for access; nothing
/// at all until something watches.
pub(super) fn note_access() {
    let Some(count) = SIGNAL.get() else { return };
    let now = access();
    let before = ACCESS.lock().replace(now);
    if before.is_some_and(|before| before != now) {
        bump(count);
    }
}

// ── What a subscription has seen ────────────────────────────────────────

/// What one subscription last saw: the resource list's fingerprint and each
/// watched URI's. A first sighting is a baseline and never reported: the
/// subscriber read it around the time it subscribed.
#[derive(Debug, Default)]
struct Seen {
    list: Option<u64>,
    resources: BTreeMap<String, u64>,
}

impl Seen {
    /// Record the list's fingerprint, and whether it differs from the last.
    fn list(&mut self, now: u64) -> bool {
        self.list.replace(now).is_some_and(|before| before != now)
    }

    /// Record `uri`'s fingerprint, and whether it differs from the last.
    fn resource(&mut self, uri: &str, now: u64) -> bool {
        self.resources
            .insert(uri.to_string(), now)
            .is_some_and(|before| before != now)
    }

    fn forget(&mut self, uri: &str) {
        self.resources.remove(uri);
    }
}

/// What a re-read found changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Changed {
    list: bool,
    resources: Vec<String>,
}

/// Re-read the resource list (when `list`) and `uris` into `seen`, and say
/// what changed. Blocking: run it on the EventKit thread.
fn refresh(seen: &mut Seen, list: bool, uris: &BTreeSet<String>) -> Changed {
    let mut changed = Changed {
        list: list && seen.list(list_fingerprint()),
        resources: Vec::new(),
    };
    for uri in uris {
        if seen.resource(uri, fingerprint(uri)) {
            changed.resources.push(uri.clone());
        }
    }
    changed
}

/// Run [`refresh`] on the EventKit thread; `None` if that thread failed.
async fn reread(seen: Arc<Mutex<Seen>>, list: bool, uris: BTreeSet<String>) -> Option<Changed> {
    eventkit_thread::run(async move {
        let mut seen = seen.lock();
        refresh(&mut seen, list, &uris)
    })
    .await
    .ok()
}

/// The resource list as a client would read it.
fn list_fingerprint() -> u64 {
    fingerprint_of(&serde_json::to_string(&resources::listed()).unwrap_or_default())
}

/// What `uri` reads now, or the error it reads as: a resource that goes away
/// (an event deleted) is a change too. Never asks for access (module docs).
fn fingerprint(uri: &str) -> u64 {
    let status = if uri.starts_with("reminders://") {
        RemindersManager::authorization_status()
    } else {
        EventsManager::authorization_status()
    };
    if status != AuthorizationStatus::FullAccess {
        return fingerprint_of(&format!("{status:?}"));
    }
    match resources::Target::parse(uri, resources::start_of_today())
        .and_then(|target| resources::read(&target, uri))
    {
        Ok(read) => fingerprint_of(&serde_json::to_string(&read.contents).unwrap_or_default()),
        Err(error) => fingerprint_of(&(error.code.0, error.message.to_string())),
    }
}

fn fingerprint_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Wait for something worth a re-read: a store change, once its burst has
/// settled, or local midnight. `false` once the count is gone, which it never
/// is: the observer holds it for the life of the process.
async fn next(signal: &mut watch::Receiver<u64>) -> bool {
    tokio::select! {
        changed = signal.changed() => {
            if changed.is_err() {
                return false;
            }
            tokio::time::sleep(SETTLE).await;
            signal.borrow_and_update();
            true
        }
        () = tokio::time::sleep(until_midnight(Local::now())) => true,
    }
}

/// From `now` until a second past the next local midnight, when a window with
/// no `from` moves on a day; an hour if the calendar has no such moment.
fn until_midnight(now: DateTime<Local>) -> Duration {
    now.date_naive()
        .checked_add_days(Days::new(1))
        .and_then(|day| day.and_hms_opt(0, 0, 1))
        .and_then(|next| Local.from_local_datetime(&next).earliest())
        .and_then(|next| (next - now).to_std().ok())
        .unwrap_or(Duration::from_secs(3600))
}

// ── 2026-07-28: one `subscriptions/listen` ──────────────────────────────

/// The part of a listen's filter this server serves: resource list changes,
/// and updates to the URIs that name a resource here. Its tools and prompts
/// never change while it runs, so it serves no list change for them.
pub(super) fn accepted(requested: &SubscriptionFilter) -> SubscriptionFilter {
    let today = resources::start_of_today();
    let ours: Vec<String> = requested
        .resource_subscriptions
        .iter()
        .flatten()
        .filter(|uri| resources::Target::parse(uri, today).is_ok())
        .cloned()
        .collect();
    let mut accepted = SubscriptionFilter::default();
    accepted.resources_list_changed = requested.resources_list_changed.filter(|&wanted| wanted);
    accepted.resource_subscriptions = (!ours.is_empty()).then_some(ours);
    accepted
}

/// Watch for one listen, as accepted, until its sink closes: a baseline first,
/// then each change. A listen with nothing to watch is held open, not ended:
/// ending it would have its client open the next one at once.
pub(super) async fn listen(sink: SubscriptionSink) {
    let list = sink.accepted().resources_list_changed == Some(true);
    let uris: BTreeSet<String> = sink
        .accepted()
        .resource_subscriptions
        .iter()
        .flatten()
        .cloned()
        .collect();
    if !list && uris.is_empty() {
        return std::future::pending().await;
    }
    let mut signal = signal();
    let seen = Arc::new(Mutex::new(Seen::default()));
    if reread(Arc::clone(&seen), list, uris.clone())
        .await
        .is_none()
    {
        return;
    }
    while next(&mut signal).await {
        let Some(changed) = reread(Arc::clone(&seen), list, uris.clone()).await else {
            return;
        };
        if changed.list && sink.notify_resource_list_changed().await.is_err() {
            return;
        }
        for uri in changed.resources {
            if sink.notify_resource_updated(uri).await.is_err() {
                return;
            }
        }
    }
}

// ── 2025-11-25: a connection and its subscriptions ──────────────────────

/// A 2025-11-25 connection's watch: every resource list change, and each URI
/// it subscribed to, told through its peer once it has initialized.
#[derive(Debug, Default)]
pub(super) struct Connection {
    uris: Mutex<BTreeSet<String>>,
    seen: Arc<Mutex<Seen>>,
    task: Mutex<Option<AbortHandle>>,
}

impl Connection {
    /// The client has initialized: watch for it until it goes.
    pub(super) fn start(self: &Arc<Self>, peer: Peer<RoleServer>) {
        let mut task = self.task.lock();
        if task.is_none() {
            let connection = Arc::clone(self);
            *task = Some(tokio::spawn(async move { connection.watch(peer).await }).abort_handle());
        }
    }

    async fn watch(&self, peer: Peer<RoleServer>) {
        let mut signal = signal();
        if reread(Arc::clone(&self.seen), true, self.uris())
            .await
            .is_none()
        {
            return;
        }
        while next(&mut signal).await {
            let Some(changed) = reread(Arc::clone(&self.seen), true, self.uris()).await else {
                return;
            };
            if changed.list && peer.notify_resource_list_changed().await.is_err() {
                return;
            }
            for uri in changed.resources {
                let updated = ResourceUpdatedNotificationParam::new(uri);
                if peer.notify_resource_updated(updated).await.is_err() {
                    return;
                }
            }
        }
    }

    fn uris(&self) -> BTreeSet<String> {
        self.uris.lock().clone()
    }

    /// Watch `uri` from now on, recording what it reads now as the baseline
    /// its updates are measured from.
    pub(super) async fn subscribe(&self, uri: String) {
        signal();
        let fresh = self.uris.lock().insert(uri.clone());
        if fresh {
            let seen = Arc::clone(&self.seen);
            let _ = eventkit_thread::run(async move {
                let now = fingerprint(&uri);
                seen.lock().resource(&uri, now);
            })
            .await;
        }
    }

    pub(super) fn unsubscribe(&self, uri: &str) {
        self.uris.lock().remove(uri);
        self.seen.lock().forget(uri);
    }

    /// The connection has closed: stop watching for it.
    pub(super) fn shutdown(&self) {
        if let Some(task) = self.task.lock().take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_reported_and_a_first_sighting_is_not() {
        let mut seen = Seen::default();
        assert!(!seen.list(1), "the first list is the baseline");
        assert!(!seen.list(1), "the same list again is no change");
        assert!(seen.list(2), "a different list is");

        assert!(!seen.resource("calendar://calendars", 7));
        assert!(!seen.resource("calendar://calendars", 7));
        assert!(seen.resource("calendar://calendars", 8));

        seen.forget("calendar://calendars");
        assert!(
            !seen.resource("calendar://calendars", 9),
            "a resource subscribed again starts from a new baseline"
        );
    }

    #[test]
    fn a_listen_is_served_resource_list_changes_and_this_servers_uris() {
        let mut requested = SubscriptionFilter::default();
        requested.tools_list_changed = Some(true);
        requested.prompts_list_changed = Some(true);
        requested.resources_list_changed = Some(true);
        requested.resource_subscriptions = Some(vec![
            "calendar://calendars".into(),
            "reminders://lists/abc?completed=true".into(),
            "email://work/INBOX".into(),
            "calendar://nope".into(),
        ]);
        let accepted = accepted(&requested);
        assert_eq!(accepted.tools_list_changed, None);
        assert_eq!(accepted.prompts_list_changed, None);
        assert_eq!(accepted.resources_list_changed, Some(true));
        assert_eq!(
            accepted.resource_subscriptions,
            Some(vec![
                "calendar://calendars".to_string(),
                "reminders://lists/abc?completed=true".to_string(),
            ])
        );

        let mut quiet = SubscriptionFilter::default();
        quiet.resources_list_changed = Some(false);
        quiet.resource_subscriptions = Some(vec!["email://work/INBOX".into()]);
        let accepted = super::accepted(&quiet);
        assert_eq!(accepted.resources_list_changed, None);
        assert_eq!(accepted.resource_subscriptions, None);
    }

    #[test]
    fn midnight_is_never_more_than_a_day_away() {
        for (h, m) in [(0, 0), (9, 30), (23, 59)] {
            let now = Local
                .with_ymd_and_hms(2026, 10, 22, h, m, 0)
                .single()
                .expect("an unambiguous local time");
            let wait = until_midnight(now);
            assert!(wait > Duration::ZERO, "{now}: {wait:?}");
            assert!(
                wait <= Duration::from_secs(24 * 3600 + 1),
                "{now}: {wait:?}"
            );
        }
    }

    /// A re-read builds managers and reads the store, so it runs on the one
    /// EventKit thread, as tools and prompts do (`mcp_thread.rs`): elsewhere it
    /// would open a store per worker, the `EKCADErrorDomain 1021` outage again.
    /// The store is read in two places, `reread` and a subscription's baseline.
    #[test]
    fn every_reread_happens_on_the_eventkit_thread() {
        let source = include_str!("mcp_changes.rs");
        let code = &source[..source.find("#[cfg(test)]").expect("the tests")];
        for reader in ["async fn reread(", "async fn subscribe("] {
            let start = code.find(reader).unwrap_or_else(|| panic!("{reader}"));
            let body = start + reader.len();
            let end = code[body..].find("fn ").map_or(code.len(), |i| body + i);
            assert!(
                code[start..end].contains("eventkit_thread::run"),
                "{reader} reads the store off the EventKit thread"
            );
        }
        assert_eq!(
            code.matches("refresh(&mut seen").count(),
            1,
            "only reread refreshes"
        );
        assert_eq!(
            code.matches("fingerprint(&uri)").count(),
            1,
            "only a subscription's baseline reads one URI"
        );
    }
}

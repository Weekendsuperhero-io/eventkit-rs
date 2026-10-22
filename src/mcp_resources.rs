//! Calendars and reminders as MCP resources: read-only, URI-addressed views
//! of what the tools reach, which a client can cache and a person can attach.
//!
//! ```text
//! calendar://calendars                                every calendar, each with its events' URI
//! calendar://events{?from,to}                         events in every calendar, in a window
//! calendar://calendars/{calendarId}/events{?from,to}  one calendar's events, in a window
//! calendar://events/{eventId}                         one event
//! reminders://lists                                   every reminder list, each with its URI
//! reminders://reminders                               open reminders in every list
//! reminders://lists/{listId}{?completed}              one list's reminders
//! reminders://reminders/{reminderId}                  one reminder
//! ```
//!
//! A window runs from `from` up to `to`, each `YYYY-MM-DD`, `YYYY-MM-DD HH:MM`
//! or `YYYY-MM-DDTHH:MM` in local time. It defaults to today and the six days
//! after, and spans at most a year. An identifier is ONE percent-encoded path
//! segment: EventKit's contain `:` and may contain `/`.
//!
//! Every read is JSON with the tools' own fields (`CalendarOutput`,
//! `EventOutput`, `ReminderOutput`), plus a `uri` on each listed item naming
//! its own resource, so a resource and a tool call never disagree about what a
//! calendar or a reminder is.
//!
//! Listing never asks for access. The four fixed resources are always listed,
//! one per calendar and per list only once full access is granted: listing
//! them sooner would put the consent dialog in front of the person when a host
//! connects, rather than when something first asks for their data.

use std::fmt::Write as _;

use chrono::{DateTime, Duration, Local, TimeZone};
use rmcp::{
    ErrorData as McpError,
    model::{Annotations, ReadResourceResult, Resource, ResourceContents, ResourceTemplate, Role},
};
use serde::Serialize;
use serde_json::{Value, json};

use super::{CalendarOutput, EventOutput, ReminderOutput, mcp_err, parse_datetime};
use crate::{
    AuthorizationStatus, EventItem, EventKitError, EventsManager, ReminderItem, RemindersManager,
};

const CALENDARS: &str = "calendar://calendars";
const EVENTS: &str = "calendar://events";
const REMINDER_LISTS: &str = "reminders://lists";
const OPEN_REMINDERS: &str = "reminders://reminders";

const EVENTS_TEMPLATE: &str = "calendar://events{?from,to}";
const CALENDAR_EVENTS_TEMPLATE: &str = "calendar://calendars/{calendarId}/events{?from,to}";
const EVENT_TEMPLATE: &str = "calendar://events/{eventId}";
const LIST_REMINDERS_TEMPLATE: &str = "reminders://lists/{listId}{?completed}";
const REMINDER_TEMPLATE: &str = "reminders://reminders/{reminderId}";

const JSON: &str = "application/json";

const WINDOW_HELP: &str = "Each end is YYYY-MM-DD, YYYY-MM-DD HH:MM or YYYY-MM-DDTHH:MM in local \
     time. The window defaults to today and the six days after, and spans at most a year.";

/// How long a read, or the resource list, may be treated as fresh: a minute.
/// Nothing announces a change, so a minute is the longest an edit made in
/// Calendar, in Reminders or through the tools should take to show.
pub(super) const FRESH_MS: u64 = 60_000;

/// The templates are compiled in, so they change only with this server.
pub(super) const TEMPLATES_FRESH_MS: u64 = 3_600_000;

/// A window that names neither end: today and the six days after.
const DEFAULT_WINDOW_DAYS: i64 = 7;

/// The widest window one read answers, so a read cannot ask EventKit for a
/// decade of events.
const MAX_WINDOW_DAYS: i64 = 366;

// ── Listing ─────────────────────────────────────────────────────────────

/// Who a resource is for: the person, to attach, as much as the agent.
fn audience() -> Annotations {
    Annotations::default().with_audience(vec![Role::User, Role::Assistant])
}

/// The four resources every client sees, listed without touching the store.
fn fixed() -> Vec<Resource> {
    vec![
        Resource::new(CALENDARS, "calendars")
            .with_title("Calendars")
            .with_description(
                "Every calendar in macOS Calendar, with the fields `list_calendars` returns and \
                 the URI of each calendar's events.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
        Resource::new(EVENTS, "upcoming-events")
            .with_title("Upcoming events")
            .with_description(
                "Events in every calendar today and the six days after. \
                 `calendar://events{?from,to}` reads another window.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
        Resource::new(REMINDER_LISTS, "reminder-lists")
            .with_title("Reminder lists")
            .with_description(
                "Every list in macOS Reminders, with the fields `list_reminder_lists` returns \
                 and the URI of each list's reminders.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
        Resource::new(OPEN_REMINDERS, "open-reminders")
            .with_title("Open reminders")
            .with_description("Every reminder not yet completed, in every list.")
            .with_mime_type(JSON)
            .with_annotations(audience()),
    ]
}

/// The fixed four, then one resource per calendar and per reminder list for
/// whichever of the two full access is already granted to. Never asks (see
/// the module docs). Blocking: the caller runs it on the EventKit thread.
pub(super) fn listed() -> Vec<Resource> {
    let mut resources = fixed();
    if EventsManager::authorization_status() == AuthorizationStatus::FullAccess
        && let Ok(calendars) = EventsManager::new().list_calendars()
    {
        resources.extend(calendars.iter().map(|calendar| {
            Resource::new(
                calendar_events_uri(&calendar.identifier),
                calendar.title.clone(),
            )
            .with_title(format!("{} (calendar)", calendar.title))
            .with_description(
                "This calendar's events today and the six days after; add \
                     `?from=…&to=…` for another window.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience())
        }));
    }
    if RemindersManager::authorization_status() == AuthorizationStatus::FullAccess
        && let Ok(lists) = RemindersManager::new().list_calendars()
    {
        resources.extend(lists.iter().map(|list| {
            Resource::new(list_uri(&list.identifier), list.title.clone())
                .with_title(format!("{} (reminders)", list.title))
                .with_description(
                    "This list's open reminders; add `?completed=true` to include completed ones.",
                )
                .with_mime_type(JSON)
                .with_annotations(audience())
        }));
    }
    resources
}

pub(super) fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(EVENTS_TEMPLATE, "events")
            .with_title("Events in a window")
            .with_description(format!(
                "Events in every calendar from `from` up to `to`. {WINDOW_HELP}"
            ))
            .with_mime_type(JSON)
            .with_annotations(audience()),
        ResourceTemplate::new(CALENDAR_EVENTS_TEMPLATE, "calendar-events")
            .with_title("One calendar's events in a window")
            .with_description(format!(
                "Events in the calendar `calendarId` (an id from `calendar://calendars`, \
                 percent-encoded) from `from` up to `to`. {WINDOW_HELP}"
            ))
            .with_mime_type(JSON)
            .with_annotations(audience()),
        ResourceTemplate::new(EVENT_TEMPLATE, "event")
            .with_title("One event")
            .with_description(
                "The event `eventId` (percent-encoded), with every field `get_event` returns.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
        ResourceTemplate::new(LIST_REMINDERS_TEMPLATE, "list-reminders")
            .with_title("One list's reminders")
            .with_description(
                "Reminders in the list `listId` (an id from `reminders://lists`, \
                 percent-encoded): open ones, or every one with `completed=true`.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
        ResourceTemplate::new(REMINDER_TEMPLATE, "reminder")
            .with_title("One reminder")
            .with_description(
                "The reminder `reminderId` (percent-encoded), with every field `get_reminder` \
                 returns.",
            )
            .with_mime_type(JSON)
            .with_annotations(audience()),
    ]
}

// ── Addressing ──────────────────────────────────────────────────────────

fn calendar_events_uri(calendar_id: &str) -> String {
    format!("{CALENDARS}/{}/events", encode_segment(calendar_id))
}

fn event_uri(event_id: &str) -> String {
    format!("{EVENTS}/{}", encode_segment(event_id))
}

fn list_uri(list_id: &str) -> String {
    format!("{REMINDER_LISTS}/{}", encode_segment(list_id))
}

fn reminder_uri(reminder_id: &str) -> String {
    format!("{OPEN_REMINDERS}/{}", encode_segment(reminder_id))
}

/// Percent-encode `text` as ONE path segment: every byte but RFC 3986's
/// unreserved characters, so an identifier's `:` or `/` cannot split it.
fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Undo percent-encoding. `None` for an escape that is not two hex digits, or
/// bytes that are not UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What a resource URI names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    Calendars,
    Events(Window),
    CalendarEvents { calendar_id: String, window: Window },
    Event(String),
    ReminderLists,
    OpenReminders,
    ListReminders { list_id: String, completed: bool },
    Reminder(String),
}

impl Target {
    /// Parse `uri`, a window with no `from` starting at `today`. A URI that
    /// names no resource here is `-32002`, the answer an unknown id gets too;
    /// a known one with a bad parameter is `-32602`, naming what was wrong.
    pub(super) fn parse(uri: &str, today: DateTime<Local>) -> Result<Self, McpError> {
        let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
        let query = Query::parse(query)?;
        if let Some(rest) = path.strip_prefix("calendar://") {
            match rest.split('/').collect::<Vec<_>>().as_slice() {
                ["calendars"] => query.none(Self::Calendars),
                ["events"] => Ok(Self::Events(query.window(today)?)),
                ["calendars", id, "events"] => Ok(Self::CalendarEvents {
                    calendar_id: segment(id, uri)?,
                    window: query.window(today)?,
                }),
                ["events", id] => query.none(Self::Event(segment(id, uri)?)),
                _ => Err(not_found(uri)),
            }
        } else if let Some(rest) = path.strip_prefix("reminders://") {
            match rest.split('/').collect::<Vec<_>>().as_slice() {
                ["lists"] => query.none(Self::ReminderLists),
                ["reminders"] => query.none(Self::OpenReminders),
                ["lists", id] => Ok(Self::ListReminders {
                    list_id: segment(id, uri)?,
                    completed: query.completed()?,
                }),
                ["reminders", id] => query.none(Self::Reminder(segment(id, uri)?)),
                _ => Err(not_found(uri)),
            }
        } else {
            Err(not_found(uri))
        }
    }
}

/// A path segment naming a calendar, list or item: decoded, never empty.
fn segment(raw: &str, uri: &str) -> Result<String, McpError> {
    match decode(raw) {
        Some(id) if !id.is_empty() => Ok(id),
        Some(_) => Err(not_found(uri)),
        None => Err(invalid(format!(
            "`{raw}` is not a valid percent-encoded identifier"
        ))),
    }
}

/// A URI's query, decoded into its pairs.
struct Query(Vec<(String, String)>);

impl Query {
    fn parse(query: &str) -> Result<Self, McpError> {
        let mut pairs = Vec::new();
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match (decode(key), decode(value)) {
                (Some(key), Some(value)) => pairs.push((key, value)),
                _ => return Err(invalid(format!("`{pair}` is not valid percent-encoding"))),
            }
        }
        Ok(Self(pairs))
    }

    /// Refuse any parameter but `allowed`, saying what this resource takes.
    fn only(&self, allowed: &[&str]) -> Result<(), McpError> {
        let Some((key, _)) = self
            .0
            .iter()
            .find(|(key, _)| !allowed.contains(&key.as_str()))
        else {
            return Ok(());
        };
        Err(invalid(if allowed.is_empty() {
            format!("`{key}` is not a parameter of this resource, which takes none")
        } else {
            format!(
                "`{key}` is not a parameter of this resource, which takes `{}`",
                allowed.join("` and `")
            )
        }))
    }

    /// `target`, when the URI carries no parameters.
    fn none(&self, target: Target) -> Result<Target, McpError> {
        self.only(&[])?;
        Ok(target)
    }

    /// The value of `key`; an empty one counts as absent, since an RFC 6570
    /// expansion of an empty variable writes `from=`.
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, v)| k == key && !v.is_empty())
            .map(|(_, v)| v.as_str())
    }

    fn window(&self, today: DateTime<Local>) -> Result<Window, McpError> {
        self.only(&["from", "to"])?;
        Window::new(self.get("from"), self.get("to"), today).map_err(invalid)
    }

    fn completed(&self) -> Result<bool, McpError> {
        self.only(&["completed"])?;
        match self.get("completed") {
            None | Some("false" | "0") => Ok(false),
            Some("true" | "1") => Ok(true),
            Some(other) => Err(invalid(format!(
                "`completed` is true or false, not `{other}`"
            ))),
        }
    }
}

/// The span a window read covers: from `from` up to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Window {
    from: DateTime<Local>,
    to: DateTime<Local>,
}

impl Window {
    fn new(from: Option<&str>, to: Option<&str>, today: DateTime<Local>) -> Result<Self, String> {
        let from = from.map(moment).transpose()?.unwrap_or(today);
        let to = to
            .map(moment)
            .transpose()?
            .unwrap_or(from + Duration::days(DEFAULT_WINDOW_DAYS));
        if to <= from {
            return Err(format!(
                "`to` ({}) must come after `from` ({})",
                to.to_rfc3339(),
                from.to_rfc3339()
            ));
        }
        if to - from > Duration::days(MAX_WINDOW_DAYS) {
            return Err(format!("a window spans at most {MAX_WINDOW_DAYS} days"));
        }
        Ok(Self { from, to })
    }
}

/// One end of a window: the tools' `YYYY-MM-DD` or `YYYY-MM-DD HH:MM`, or the
/// `T` form a URI more often carries.
fn moment(text: &str) -> Result<DateTime<Local>, String> {
    parse_datetime(&text.replacen('T', " ", 1))
}

/// Midnight today, local time: where a window with no `from` begins (now, on
/// a day whose midnight a clock change skips).
pub(super) fn start_of_today() -> DateTime<Local> {
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
        .unwrap_or(now)
}

// ── Reading ─────────────────────────────────────────────────────────────

/// Read `target` from the store as `uri`'s contents. Blocking: the caller runs
/// it on the EventKit thread.
pub(super) fn read(target: &Target, uri: &str) -> Result<ReadResourceResult, McpError> {
    let fail = |error: EventKitError| store_err(uri, &error);
    let body = match target {
        Target::Calendars => {
            let calendars = EventsManager::new().list_calendars().map_err(fail)?;
            let items = calendars
                .iter()
                .map(|c| {
                    linked(
                        &CalendarOutput::from_info(c),
                        calendar_events_uri(&c.identifier),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            json!({ "count": items.len(), "items": items })
        }
        Target::Events(window) => {
            let events = EventsManager::new()
                .fetch_events(window.from, window.to, None)
                .map_err(fail)?;
            window_body(window, &events)?
        }
        Target::CalendarEvents {
            calendar_id,
            window,
        } => {
            let manager = EventsManager::new();
            let calendar = manager
                .list_calendars()
                .map_err(fail)?
                .into_iter()
                .find(|c| &c.identifier == calendar_id)
                .ok_or_else(|| not_found(uri))?;
            let events: Vec<_> = manager
                .fetch_events(window.from, window.to, None)
                .map_err(fail)?
                .into_iter()
                .filter(|e| e.calendar_id.as_deref() == Some(calendar_id.as_str()))
                .collect();
            let mut body = window_body(window, &events)?;
            body["calendar"] = to_json(&CalendarOutput::from_info(&calendar))?;
            body
        }
        Target::Event(event_id) => {
            let manager = EventsManager::new();
            let event = manager.get_event(event_id).map_err(fail)?;
            to_json(&EventOutput::from_item(&event, &manager))?
        }
        Target::ReminderLists => {
            let lists = RemindersManager::new().list_calendars().map_err(fail)?;
            let items = lists
                .iter()
                .map(|l| linked(&CalendarOutput::from_info(l), list_uri(&l.identifier)))
                .collect::<Result<Vec<_>, _>>()?;
            json!({ "count": items.len(), "items": items })
        }
        Target::OpenReminders => {
            let reminders = RemindersManager::new()
                .fetch_incomplete_reminders()
                .map_err(fail)?;
            reminders_body(&reminders)?
        }
        Target::ListReminders { list_id, completed } => {
            let manager = RemindersManager::new();
            let list = manager
                .list_calendars()
                .map_err(fail)?
                .into_iter()
                .find(|l| &l.identifier == list_id)
                .ok_or_else(|| not_found(uri))?;
            let reminders = manager
                .fetch_list_reminders(list_id, *completed)
                .map_err(fail)?;
            let mut body = reminders_body(&reminders)?;
            body["list"] = to_json(&CalendarOutput::from_info(&list))?;
            body["completed"] = Value::Bool(*completed);
            body
        }
        Target::Reminder(reminder_id) => {
            let manager = RemindersManager::new();
            let reminder = manager.get_reminder(reminder_id).map_err(fail)?;
            to_json(&ReminderOutput::from_item(&reminder, &manager))?
        }
    };
    let text = serde_json::to_string_pretty(&body)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(text, uri).with_mime_type(JSON),
    ]))
}

fn window_body(window: &Window, events: &[EventItem]) -> Result<Value, McpError> {
    let items = events
        .iter()
        .map(|e| linked(&EventOutput::from_item_summary(e), event_uri(&e.identifier)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "from": window.from.to_rfc3339(),
        "to": window.to.to_rfc3339(),
        "count": items.len(),
        "items": items,
    }))
}

fn reminders_body(reminders: &[ReminderItem]) -> Result<Value, McpError> {
    let items = reminders
        .iter()
        .map(|r| {
            linked(
                &ReminderOutput::from_item_summary(r),
                reminder_uri(&r.identifier),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "count": items.len(), "items": items }))
}

/// `output`'s fields with `uri`, the resource it is, beside them.
fn linked(output: &impl Serialize, uri: String) -> Result<Value, McpError> {
    let mut value = to_json(output)?;
    if let Value::Object(fields) = &mut value {
        fields.insert("uri".to_string(), Value::String(uri));
    }
    Ok(value)
}

fn to_json(output: &impl Serialize) -> Result<Value, McpError> {
    serde_json::to_value(output).map_err(|e| McpError::internal_error(e.to_string(), None))
}

/// A store error as a read's: a calendar, list or item that is not there is
/// the resource not existing (`-32002`); anything else is the tools' error,
/// remediation hint and all.
fn store_err(uri: &str, error: &EventKitError) -> McpError {
    match error {
        EventKitError::CalendarNotFound(_) | EventKitError::ItemNotFound(_) => not_found(uri),
        other => mcp_err(other),
    }
}

fn not_found(uri: &str) -> McpError {
    McpError::resource_not_found(
        format!("Resource not found: {uri}"),
        Some(json!({ "uri": uri })),
    )
}

fn invalid(message: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(message.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorCode;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("an unambiguous local time")
    }

    fn today() -> DateTime<Local> {
        at(2026, 10, 22, 0, 0)
    }

    fn parse(uri: &str) -> Result<Target, McpError> {
        Target::parse(uri, today())
    }

    fn week() -> Window {
        Window {
            from: today(),
            to: at(2026, 10, 29, 0, 0),
        }
    }

    #[test]
    fn every_fixed_resource_reads_back_as_what_it_names() {
        let targets: Vec<_> = fixed()
            .iter()
            .map(|r| parse(&r.uri).expect("a listed URI parses"))
            .collect();
        assert_eq!(
            targets,
            [
                Target::Calendars,
                Target::Events(week()),
                Target::ReminderLists,
                Target::OpenReminders,
            ]
        );
    }

    #[test]
    fn each_template_names_the_target_its_filled_form_parses_to() {
        let templates: Vec<_> = templates().iter().map(|t| t.uri_template.clone()).collect();
        assert_eq!(
            templates,
            [
                EVENTS_TEMPLATE,
                CALENDAR_EVENTS_TEMPLATE,
                EVENT_TEMPLATE,
                LIST_REMINDERS_TEMPLATE,
                REMINDER_TEMPLATE,
            ]
        );
        let window = Window {
            from: at(2026, 11, 1, 0, 0),
            to: at(2026, 11, 2, 9, 30),
        };
        assert_eq!(
            parse("calendar://events?from=2026-11-01&to=2026-11-02T09:30").unwrap(),
            Target::Events(window)
        );
        assert_eq!(
            parse("calendar://calendars/cal-1/events?from=2026-11-01&to=2026-11-02%2009:30")
                .unwrap(),
            Target::CalendarEvents {
                calendar_id: "cal-1".into(),
                window,
            }
        );
        assert_eq!(
            parse("calendar://events/ev-1").unwrap(),
            Target::Event("ev-1".into())
        );
        assert_eq!(
            parse("reminders://lists/list-1?completed=true").unwrap(),
            Target::ListReminders {
                list_id: "list-1".into(),
                completed: true,
            }
        );
        assert_eq!(
            parse("reminders://reminders/rem-1").unwrap(),
            Target::Reminder("rem-1".into())
        );
    }

    /// EventKit identifiers carry `:` (an event's is its calendar item's id and
    /// its occurrence's), and nothing stops a `/`: one segment either way.
    #[test]
    fn an_identifier_survives_its_uri() {
        for id in ["6D2F0C1A-5B9E:4C3D2B1A", "a/b c%d", "Café ☕", "-._~"] {
            assert_eq!(parse(&event_uri(id)).unwrap(), Target::Event(id.into()));
            assert_eq!(
                parse(&reminder_uri(id)).unwrap(),
                Target::Reminder(id.into())
            );
            assert_eq!(
                parse(&list_uri(id)).unwrap(),
                Target::ListReminders {
                    list_id: id.into(),
                    completed: false,
                }
            );
            assert_eq!(
                parse(&calendar_events_uri(id)).unwrap(),
                Target::CalendarEvents {
                    calendar_id: id.into(),
                    window: week(),
                }
            );
        }
    }

    #[test]
    fn a_window_defaults_to_a_week_and_takes_either_end_alone() {
        assert_eq!(parse("calendar://events").unwrap(), Target::Events(week()));
        assert_eq!(
            parse("calendar://events?from=&to=").unwrap(),
            Target::Events(week()),
            "an empty expansion is an absent parameter"
        );
        assert_eq!(
            parse("calendar://events?from=2026-12-01").unwrap(),
            Target::Events(Window {
                from: at(2026, 12, 1, 0, 0),
                to: at(2026, 12, 8, 0, 0),
            })
        );
        assert_eq!(
            parse("calendar://events?to=2026-10-23%2012:00").unwrap(),
            Target::Events(Window {
                from: today(),
                to: at(2026, 10, 23, 12, 0),
            })
        );
    }

    fn code(result: Result<Target, McpError>) -> ErrorCode {
        result.expect_err("refused").code
    }

    #[test]
    fn a_window_runs_forwards_and_within_a_year() {
        for uri in [
            "calendar://events?from=2026-10-29&to=2026-10-22",
            "calendar://events?from=2026-10-22&to=2026-10-22",
            "calendar://events?from=2026-01-01&to=2027-06-01",
            "calendar://events?from=yesterday",
        ] {
            assert_eq!(code(parse(uri)), ErrorCode::INVALID_PARAMS, "{uri}");
        }
    }

    #[test]
    fn completed_is_a_flag_that_defaults_to_open_reminders() {
        for (query, completed) in [
            ("", false),
            ("?completed=false", false),
            ("?completed=0", false),
            ("?completed=true", true),
            ("?completed=1", true),
        ] {
            assert_eq!(
                parse(&format!("reminders://lists/l{query}")).unwrap(),
                Target::ListReminders {
                    list_id: "l".into(),
                    completed,
                }
            );
        }
        assert_eq!(
            code(parse("reminders://lists/l?completed=maybe")),
            ErrorCode::INVALID_PARAMS
        );
    }

    /// Unknown is `-32002`, like an id that names nothing; a known resource
    /// asked for wrongly is `-32602`, so the agent fixes the parameter instead
    /// of concluding the calendar is gone.
    #[test]
    fn what_is_not_here_is_not_found_and_what_is_asked_wrongly_is_invalid() {
        for uri in [
            "bridge://calendars",
            "calendar://",
            "calendar://nope",
            "calendar://calendars/x",
            "calendar://events/",
            "reminders://lists/",
            "reminders://lists/a/b",
        ] {
            assert_eq!(code(parse(uri)), ErrorCode::RESOURCE_NOT_FOUND, "{uri}");
        }
        for uri in [
            "calendar://calendars?from=2026-10-22",
            "calendar://events?start=2026-10-22",
            "calendar://events/x?completed=true",
            "reminders://reminders?completed=true",
            "reminders://lists/%zz",
            "calendar://events?from=%2",
        ] {
            assert_eq!(code(parse(uri)), ErrorCode::INVALID_PARAMS, "{uri}");
        }
    }

    #[test]
    fn a_wrong_parameter_is_named_with_what_the_resource_takes() {
        let message = parse("calendar://events?start=2026-10-22")
            .expect_err("refused")
            .message;
        assert!(
            message.contains("`start`") && message.contains("`from` and `to`"),
            "{message}"
        );
    }
}

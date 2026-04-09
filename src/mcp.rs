//! EventKit MCP Server
//!
//! A Model Context Protocol (MCP) server that exposes macOS Calendar and Reminders
//! functionality via the EventKit framework.
//!
//! This module is gated behind the `mcp` feature flag.

use rmcp::{
    ErrorData as McpError, RoleServer, ServiceExt,
    handler::server::{
        router::{prompt::PromptRouter, tool::ToolRouter},
        wrapper::Parameters,
    },
    model::*,
    prompt, prompt_handler, prompt_router, schemars,
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};

use crate::{EventsManager, RemindersManager};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};

use rmcp::handler::server::wrapper::Json;

// ============================================================================
// Structured Output Types
// ============================================================================

/// Convert an EventKitError into an McpError for tool returns.
fn mcp_err(e: &crate::EventKitError) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

fn mcp_invalid(msg: impl std::fmt::Display) -> McpError {
    McpError::invalid_params(msg.to_string(), None)
}

#[derive(Serialize, JsonSchema)]
struct ListResponse<T: Serialize> {
    #[schemars(with = "i64")]
    count: usize,
    items: Vec<T>,
}

#[derive(Serialize, JsonSchema)]
struct DeletedResponse {
    id: String,
}

#[derive(Serialize, JsonSchema)]
struct BatchResponse {
    #[schemars(with = "i64")]
    total: usize,
    #[schemars(with = "i64")]
    succeeded: usize,
    #[schemars(with = "i64")]
    failed: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<BatchItemError>,
}

#[derive(Serialize, JsonSchema)]
struct BatchItemError {
    item_id: String,
    message: String,
}

#[derive(Serialize, JsonSchema)]
struct SearchResponse {
    query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reminders: Option<ListResponse<ReminderOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    events: Option<ListResponse<EventOutput>>,
}

#[derive(Serialize, JsonSchema)]
struct CoordinateOutput {
    latitude: f64,
    longitude: f64,
}

#[derive(Serialize, JsonSchema)]
struct LocationOutput {
    title: String,
    latitude: f64,
    longitude: f64,
    radius_meters: f64,
}

#[derive(Serialize, JsonSchema)]
struct AlarmOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    relative_offset_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    absolute_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proximity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<LocationOutput>,
}

impl AlarmOutput {
    fn from_info(a: &crate::AlarmInfo) -> Self {
        Self {
            relative_offset_seconds: a.relative_offset,
            absolute_date: a.absolute_date.map(|d| d.to_rfc3339()),
            proximity: match a.proximity {
                crate::AlarmProximity::Enter => Some("enter".into()),
                crate::AlarmProximity::Leave => Some("leave".into()),
                crate::AlarmProximity::None => None,
            },
            location: a.location.as_ref().map(|l| LocationOutput {
                title: l.title.clone(),
                latitude: l.latitude,
                longitude: l.longitude,
                radius_meters: l.radius,
            }),
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct RecurrenceRuleOutput {
    frequency: String,
    #[schemars(with = "i64")]
    interval: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<Vec<i32>>")]
    days_of_week: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    days_of_month: Option<Vec<i32>>,
    end: RecurrenceEndOutput,
}

#[derive(Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecurrenceEndOutput {
    Never,
    AfterCount {
        #[schemars(with = "i64")]
        count: usize,
    },
    OnDate { date: String },
}

impl RecurrenceRuleOutput {
    fn from_rule(r: &crate::RecurrenceRule) -> Self {
        Self {
            frequency: match r.frequency {
                crate::RecurrenceFrequency::Daily => "daily",
                crate::RecurrenceFrequency::Weekly => "weekly",
                crate::RecurrenceFrequency::Monthly => "monthly",
                crate::RecurrenceFrequency::Yearly => "yearly",
            }
            .into(),
            interval: r.interval,
            days_of_week: r.days_of_week.clone(),
            days_of_month: r.days_of_month.clone(),
            end: match &r.end {
                crate::RecurrenceEndCondition::Never => RecurrenceEndOutput::Never,
                crate::RecurrenceEndCondition::AfterCount(n) => {
                    RecurrenceEndOutput::AfterCount { count: *n }
                }
                crate::RecurrenceEndCondition::OnDate(d) => RecurrenceEndOutput::OnDate {
                    date: d.to_rfc3339(),
                },
            },
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct AttendeeOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    role: String,
    status: String,
    is_current_user: bool,
}

impl AttendeeOutput {
    fn from_info(p: &crate::ParticipantInfo) -> Self {
        Self {
            name: p.name.clone(),
            role: format!("{:?}", p.role).to_lowercase(),
            status: format!("{:?}", p.status).to_lowercase(),
            is_current_user: p.is_current_user,
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct ReminderOutput {
    id: String,
    title: String,
    completed: bool,
    priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    alarms: Vec<AlarmOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recurrence_rules: Vec<RecurrenceRuleOutput>,
}

impl ReminderOutput {
    fn from_item(r: &crate::ReminderItem, manager: &RemindersManager) -> Self {
        let alarms = if r.has_alarms {
            manager
                .get_alarms(&r.identifier)
                .unwrap_or_default()
                .iter()
                .map(AlarmOutput::from_info)
                .collect()
        } else {
            vec![]
        };
        let recurrence_rules = if r.has_recurrence_rules {
            manager
                .get_recurrence_rules(&r.identifier)
                .unwrap_or_default()
                .iter()
                .map(RecurrenceRuleOutput::from_rule)
                .collect()
        } else {
            vec![]
        };
        Self {
            tags: r.notes.as_deref().map(extract_tags).unwrap_or_default(),
            alarms,
            recurrence_rules,
            ..Self::from_item_summary(r)
        }
    }

    fn from_item_summary(r: &crate::ReminderItem) -> Self {
        Self {
            id: r.identifier.clone(),
            title: r.title.clone(),
            completed: r.completed,
            priority: Priority::label(r.priority).into(),
            list_name: r.calendar_title.clone(),
            list_id: r.calendar_id.clone(),
            due_date: r.due_date.map(|d| d.to_rfc3339()),
            start_date: r.start_date.map(|d| d.to_rfc3339()),
            completion_date: r.completion_date.map(|d| d.to_rfc3339()),
            notes: r.notes.clone(),
            url: r.url.clone(),
            tags: r.notes.as_deref().map(extract_tags).unwrap_or_default(),
            alarms: vec![],
            recurrence_rules: vec![],
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct EventOutput {
    id: String,
    title: String,
    start: String,
    end: String,
    all_day: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    calendar_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    calendar_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    availability: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    structured_location: Option<LocationOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    alarms: Vec<AlarmOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recurrence_rules: Vec<RecurrenceRuleOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attendees: Vec<AttendeeOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    organizer: Option<AttendeeOutput>,
    is_detached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    occurrence_date: Option<String>,
}

impl EventOutput {
    fn from_item(e: &crate::EventItem, manager: &EventsManager) -> Self {
        let alarms = manager
            .get_event_alarms(&e.identifier)
            .unwrap_or_default()
            .iter()
            .map(AlarmOutput::from_info)
            .collect();
        let recurrence_rules = manager
            .get_event_recurrence_rules(&e.identifier)
            .unwrap_or_default()
            .iter()
            .map(RecurrenceRuleOutput::from_rule)
            .collect();
        Self {
            alarms,
            recurrence_rules,
            ..Self::from_item_summary(e)
        }
    }

    fn from_item_summary(e: &crate::EventItem) -> Self {
        Self {
            id: e.identifier.clone(),
            title: e.title.clone(),
            start: e.start_date.to_rfc3339(),
            end: e.end_date.to_rfc3339(),
            all_day: e.all_day,
            calendar_name: e.calendar_title.clone(),
            calendar_id: e.calendar_id.clone(),
            notes: e.notes.clone(),
            location: e.location.clone(),
            url: e.url.clone(),
            availability: match e.availability {
                crate::EventAvailability::Busy => "busy",
                crate::EventAvailability::Free => "free",
                crate::EventAvailability::Tentative => "tentative",
                crate::EventAvailability::Unavailable => "unavailable",
                _ => "not_supported",
            }
            .into(),
            status: match e.status {
                crate::EventStatus::Confirmed => "confirmed",
                crate::EventStatus::Tentative => "tentative",
                crate::EventStatus::Canceled => "canceled",
                _ => "none",
            }
            .into(),
            structured_location: e.structured_location.as_ref().map(|l| LocationOutput {
                title: l.title.clone(),
                latitude: l.latitude,
                longitude: l.longitude,
                radius_meters: l.radius,
            }),
            alarms: vec![],
            recurrence_rules: vec![],
            attendees: e.attendees.iter().map(AttendeeOutput::from_info).collect(),
            organizer: e.organizer.as_ref().map(AttendeeOutput::from_info),
            is_detached: e.is_detached,
            occurrence_date: e.occurrence_date.map(|d| d.to_rfc3339()),
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct CalendarOutput {
    id: String,
    title: String,
    color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_id: Option<String>,
    allows_modifications: bool,
    is_immutable: bool,
    is_subscribed: bool,
    entity_types: Vec<String>,
}

impl CalendarOutput {
    fn from_info(c: &crate::CalendarInfo) -> Self {
        Self {
            id: c.identifier.clone(),
            title: c.title.clone(),
            color: c
                .color
                .map(|(r, g, b, _)| CalendarColor::from_rgba(r, g, b).to_string())
                .unwrap_or_else(|| "none".into()),
            source: c.source.clone(),
            source_id: c.source_id.clone(),
            allows_modifications: c.allows_modifications,
            is_immutable: c.is_immutable,
            is_subscribed: c.is_subscribed,
            entity_types: c.allowed_entity_types.clone(),
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct SourceOutput {
    id: String,
    title: String,
    source_type: String,
}

impl SourceOutput {
    fn from_info(s: &crate::SourceInfo) -> Self {
        Self {
            id: s.identifier.clone(),
            title: s.title.clone(),
            source_type: s.source_type.clone(),
        }
    }
}

// ============================================================================
// Shared Enums
// ============================================================================

/// Priority level for reminders.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// No priority (0)
    None,
    /// Low priority (9)
    Low,
    /// Medium priority (5)
    Medium,
    /// High priority (1) — also shows as "flagged" in Reminders.app
    High,
}

impl Priority {
    fn to_usize(&self) -> usize {
        match self {
            Priority::None => 0,
            Priority::Low => 9,
            Priority::Medium => 5,
            Priority::High => 1,
        }
    }

    fn label(v: usize) -> &'static str {
        match v {
            1..=4 => "high",
            5 => "medium",
            6..=9 => "low",
            _ => "none",
        }
    }
}

/// Item type discriminator for unified tools.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ItemType {
    Reminder,
    Event,
}

// ============================================================================
// Inline Alarm & Recurrence Param Types
// ============================================================================

/// Alarm configuration for inline use in create/update.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AlarmParam {
    /// Offset in seconds before the due date (negative = before, e.g., -600 = 10 minutes before)
    pub relative_offset: Option<f64>,
    /// Proximity trigger: "enter" or "leave" (for location-based alarms on reminders)
    pub proximity: Option<String>,
    /// Title of the location for geofenced alarms
    pub location_title: Option<String>,
    /// Latitude of the location
    pub latitude: Option<f64>,
    /// Longitude of the location
    pub longitude: Option<f64>,
    /// Geofence radius in meters (default: 100)
    pub radius: Option<f64>,
}

/// Recurrence configuration for inline use in create/update.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RecurrenceParam {
    /// Frequency: "daily", "weekly", "monthly", or "yearly"
    pub frequency: String,
    /// Repeat every N intervals (e.g., 2 = every 2 weeks). Default: 1
    #[serde(default = "default_interval")]
    #[schemars(with = "i64")]
    pub interval: usize,
    /// Days of the week (1=Sun, 2=Mon, ..., 7=Sat) for weekly/monthly rules
    #[schemars(with = "Option<Vec<i32>>")]
    pub days_of_week: Option<Vec<u8>>,
    /// Days of the month (1-31) for monthly rules
    pub days_of_month: Option<Vec<i32>>,
    /// End after this many occurrences (mutually exclusive with end_date)
    #[schemars(with = "Option<i64>")]
    pub end_after_count: Option<usize>,
    /// End on this date in format 'YYYY-MM-DD' (mutually exclusive with end_after_count)
    pub end_date: Option<String>,
}

// ============================================================================
// Request/Response Types for Tools
// ============================================================================

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListRemindersRequest {
    /// If true, show all reminders including completed ones. Default: false
    #[serde(default)]
    pub show_completed: bool,
    /// Optional: Filter to a specific reminder list by name
    pub list_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateReminderRequest {
    /// The title/name of the reminder
    pub title: String,
    /// The name of the reminder list to add to (REQUIRED - use list_reminder_lists to see available lists)
    pub list_name: String,
    /// Optional notes/description for the reminder
    pub notes: Option<String>,
    /// Priority: "none", "low", "medium", "high" (high = flagged)
    pub priority: Option<Priority>,
    /// Optional due date in format 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'. If only time 'HH:MM' is given, today's date is used.
    pub due_date: Option<String>,
    /// Optional start date when to begin working (format: 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM')
    pub start_date: Option<String>,
    /// Optional URL to associate with the reminder
    pub url: Option<String>,
    /// Optional alarms (replaces all existing). Each alarm can be time-based or location-based.
    pub alarms: Option<Vec<AlarmParam>>,
    /// Optional recurrence rule (replaces existing)
    pub recurrence: Option<RecurrenceParam>,
    /// Optional tags (stored as #tagname in notes). Replaces existing tags when provided.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateReminderRequest {
    /// The unique identifier of the reminder to update
    pub reminder_id: String,
    /// The name of the reminder list to move this reminder to
    pub list_name: Option<String>,
    /// New title for the reminder
    pub title: Option<String>,
    /// New notes for the reminder
    pub notes: Option<String>,
    /// Mark as completed (true) or incomplete (false)
    pub completed: Option<bool>,
    /// Priority: "none", "low", "medium", "high" (high = flagged)
    pub priority: Option<Priority>,
    /// New due date in format 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'. Set to empty string to clear.
    pub due_date: Option<String>,
    /// New start date. Set to empty string to clear.
    pub start_date: Option<String>,
    /// URL to associate (set to empty string to clear)
    pub url: Option<String>,
    /// Alarms (replaces all existing when provided). Pass empty array to clear.
    pub alarms: Option<Vec<AlarmParam>>,
    /// Recurrence rule (replaces existing when provided). Omit to keep, set frequency to "" to clear.
    pub recurrence: Option<RecurrenceParam>,
    /// Tags (stored as #tagname in notes). Replaces existing tags when provided.
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateReminderListRequest {
    /// The name of the new reminder list to create
    pub name: String,
}

/// Predefined colors for calendars and reminder lists.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CalendarColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Brown,
    Pink,
    Teal,
}

impl CalendarColor {
    fn to_rgba(&self) -> (f64, f64, f64, f64) {
        match self {
            CalendarColor::Red => (1.0, 0.231, 0.188, 1.0),
            CalendarColor::Orange => (1.0, 0.584, 0.0, 1.0),
            CalendarColor::Yellow => (1.0, 0.8, 0.0, 1.0),
            CalendarColor::Green => (0.298, 0.851, 0.392, 1.0),
            CalendarColor::Blue => (0.0, 0.478, 1.0, 1.0),
            CalendarColor::Purple => (0.686, 0.322, 0.871, 1.0),
            CalendarColor::Brown => (0.635, 0.518, 0.369, 1.0),
            CalendarColor::Pink => (1.0, 0.176, 0.333, 1.0),
            CalendarColor::Teal => (0.353, 0.784, 0.98, 1.0),
        }
    }

    /// Find the closest named color for an RGBA value.
    fn from_rgba(r: f64, g: f64, b: f64) -> &'static str {
        let colors: &[(&str, (f64, f64, f64))] = &[
            ("red", (1.0, 0.231, 0.188)),
            ("orange", (1.0, 0.584, 0.0)),
            ("yellow", (1.0, 0.8, 0.0)),
            ("green", (0.298, 0.851, 0.392)),
            ("blue", (0.0, 0.478, 1.0)),
            ("purple", (0.686, 0.322, 0.871)),
            ("brown", (0.635, 0.518, 0.369)),
            ("pink", (1.0, 0.176, 0.333)),
            ("teal", (0.353, 0.784, 0.98)),
        ];
        colors
            .iter()
            .min_by(|(_, a), (_, b_)| {
                let da = (a.0 - r).powi(2) + (a.1 - g).powi(2) + (a.2 - b).powi(2);
                let db = (b_.0 - r).powi(2) + (b_.1 - g).powi(2) + (b_.2 - b).powi(2);
                da.partial_cmp(&db).unwrap()
            })
            .map(|(name, _)| *name)
            .unwrap_or("blue")
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateReminderListRequest {
    /// The unique identifier of the reminder list to update
    pub list_id: String,
    /// New name for the list (optional)
    pub name: Option<String>,
    /// Color for the list (optional). Use a color name or custom hex.
    pub color: Option<CalendarColor>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateEventCalendarRequest {
    /// The unique identifier of the event calendar to update
    pub calendar_id: String,
    /// New name for the calendar (optional)
    pub name: Option<String>,
    /// Color for the calendar (optional). Use a color name or custom hex.
    pub color: Option<CalendarColor>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeleteReminderListRequest {
    /// The unique identifier of the reminder list to delete
    pub list_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReminderIdRequest {
    /// The unique identifier of the reminder
    pub reminder_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListEventsRequest {
    /// Number of days from today to include (default: 1 for today only)
    #[serde(default = "default_days")]
    pub days: i64,
    /// Optional: Filter to a specific calendar by ID (use list_calendars to get IDs)
    pub calendar_id: Option<String>,
}

fn default_days() -> i64 {
    1
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateEventRequest {
    /// The title of the event
    pub title: String,
    /// Start date/time in format 'YYYY-MM-DD HH:MM' or 'YYYY-MM-DD' for all-day events
    pub start: String,
    /// End date/time in format 'YYYY-MM-DD HH:MM'. If not specified, uses duration_minutes.
    pub end: Option<String>,
    /// Duration in minutes (default: 60). Used if end is not specified.
    #[serde(default = "default_duration")]
    pub duration_minutes: i64,
    /// Optional notes/description for the event
    pub notes: Option<String>,
    /// Optional location for the event
    pub location: Option<String>,
    /// Optional: The name of the calendar to add to
    pub calendar_name: Option<String>,
    /// Whether this is an all-day event
    #[serde(default)]
    pub all_day: bool,
    /// Optional URL to associate with the event
    pub url: Option<String>,
    /// Optional alarms (replaces all existing). Time-based only for events.
    pub alarms: Option<Vec<AlarmParam>>,
    /// Optional recurrence rule
    pub recurrence: Option<RecurrenceParam>,
}

fn default_duration() -> i64 {
    60
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EventIdRequest {
    /// The unique identifier of the event
    pub event_id: String,
    /// If true, affect this event and all future occurrences (for recurring events)
    #[serde(default)]
    pub affect_future: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateEventRequest {
    /// The unique identifier of the event to update
    pub event_id: String,
    /// New title for the event
    pub title: Option<String>,
    /// New notes for the event
    pub notes: Option<String>,
    /// New location for the event
    pub location: Option<String>,
    /// New start date/time in format 'YYYY-MM-DD HH:MM'
    pub start: Option<String>,
    /// New end date/time in format 'YYYY-MM-DD HH:MM'
    pub end: Option<String>,
    /// URL to associate (set to empty string to clear)
    pub url: Option<String>,
    /// Alarms (replaces all existing when provided). Pass empty array to clear.
    pub alarms: Option<Vec<AlarmParam>>,
    /// Recurrence rule (replaces existing when provided)
    pub recurrence: Option<RecurrenceParam>,
}

// ============================================================================
// Batch Operation Request Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BatchDeleteRequest {
    /// Whether these are "reminder" or "event" items
    pub item_type: ItemType,
    /// List of item IDs to delete
    pub item_ids: Vec<String>,
    /// For recurring events: if true, delete this and all future occurrences
    #[serde(default)]
    pub affect_future: bool,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BatchMoveRequest {
    /// List of reminder IDs to move
    pub reminder_ids: Vec<String>,
    /// The name of the destination reminder list
    pub destination_list_name: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BatchUpdateItem {
    /// The unique identifier of the item to update
    pub item_id: String,
    /// New title
    pub title: Option<String>,
    /// New notes
    pub notes: Option<String>,
    /// Mark completed (reminders only)
    pub completed: Option<bool>,
    /// Priority (reminders only)
    pub priority: Option<Priority>,
    /// Due date (reminders only)
    pub due_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BatchUpdateRequest {
    /// Whether these are "reminder" or "event" items
    pub item_type: ItemType,
    /// List of updates to apply
    pub updates: Vec<BatchUpdateItem>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchRequest {
    /// Text to search for in titles and notes (case-insensitive)
    pub query: String,
    /// Whether to search "reminder" or "event" items. If omitted, searches both.
    pub item_type: Option<ItemType>,
    /// For reminders: if true, also search completed reminders. Default: false
    #[serde(default)]
    pub include_completed: bool,
    /// For events: number of days from today to search (default: 30)
    #[serde(default = "default_search_days")]
    pub days: i64,
}

fn default_search_days() -> i64 {
    30
}

fn default_interval() -> usize {
    1
}

// ============================================================================
// Prompt Argument Types
// ============================================================================

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListRemindersPromptArgs {
    /// Name of the reminder list to show. If not provided, shows all lists.
    #[serde(default)]
    pub list_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MoveReminderPromptArgs {
    /// The unique identifier of the reminder to move
    pub reminder_id: String,
    /// The name of the destination reminder list
    pub destination_list: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateReminderPromptArgs {
    /// Title of the reminder
    pub title: String,
    /// Detailed notes/context for the reminder
    #[serde(default)]
    pub notes: Option<String>,
    /// Name of the reminder list to add to
    #[serde(default)]
    pub list_name: Option<String>,
    /// Priority (0 = none, 1-4 = high, 5 = medium, 6-9 = low)
    #[serde(default)]
    #[schemars(with = "Option<i32>")]
    pub priority: Option<u8>,
    /// Due date in format "YYYY-MM-DD" or "YYYY-MM-DD HH:MM"
    #[serde(default)]
    pub due_date: Option<String>,
}

// ============================================================================
// EventKit MCP Server
// ============================================================================

/// EventKit MCP Server - provides access to macOS Calendar and Reminders
/// Note: EventKit managers are created fresh in each tool call as they are not Send+Sync
#[derive(Clone)]
pub struct EventKitServer {
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
    /// Limits concurrent EventKit access to 1 at a time, since EventKit is !Send+!Sync
    concurrency: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Default for EventKitServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a date string in format "YYYY-MM-DD" or "YYYY-MM-DD HH:MM"
fn parse_datetime(s: &str) -> Result<DateTime<Local>, String> {
    // Try parsing with time first
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local
            .from_local_datetime(&dt)
            .single()
            .ok_or_else(|| "Invalid local datetime".to_string());
    }

    // Try parsing date only
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| "Invalid date".to_string())?;
        return Local
            .from_local_datetime(&dt)
            .single()
            .ok_or_else(|| "Invalid local datetime".to_string());
    }

    Err("Invalid date format. Use 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'".to_string())
}

#[tool_router]
impl EventKitServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
            concurrency: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    // ========================================================================
    // Reminders Tools
    // ========================================================================

    #[tool(description = "List all reminder lists (calendars) available in macOS Reminders.")]
    async fn list_reminder_lists(&self) -> Result<Json<ListResponse<CalendarOutput>>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.list_calendars() {
            Ok(lists) => {
                let items: Vec<_> = lists.iter().map(CalendarOutput::from_info).collect();
                Ok(Json(ListResponse {
                    count: items.len(),
                    items,
                }))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "List reminders from macOS Reminders app. Can filter by completion status."
    )]
    async fn list_reminders(
        &self,
        Parameters(params): Parameters<ListRemindersRequest>,
    ) -> Result<Json<ListResponse<ReminderOutput>>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        let reminders = if params.show_completed {
            manager.fetch_all_reminders()
        } else {
            manager.fetch_incomplete_reminders()
        };

        match reminders {
            Ok(items) => {
                let filtered: Vec<_> = if let Some(name) = params.list_name {
                    items
                        .into_iter()
                        .filter(|r| r.calendar_title.as_deref() == Some(&name))
                        .collect()
                } else {
                    items
                };
                let items: Vec<_> = filtered
                    .iter()
                    .map(ReminderOutput::from_item_summary)
                    .collect();
                Ok(Json(ListResponse {
                    count: items.len(),
                    items,
                }))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "Create a new reminder in macOS Reminders. You MUST specify which list to add it to. Use list_reminder_lists first to see available lists. Can include alarms, recurrence, and URL inline."
    )]
    async fn create_reminder(
        &self,
        Parameters(params): Parameters<CreateReminderRequest>,
    ) -> Result<Json<ReminderOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        // Validate the list exists
        let calendar_title = match manager.list_calendars() {
            Ok(lists) => {
                if let Some(cal) = lists.iter().find(|c| c.title == params.list_name) {
                    cal.title.clone()
                } else {
                    let available: Vec<_> = lists.iter().map(|c| c.title.as_str()).collect();
                    return Err(mcp_invalid(format!(
                        "List '{}' not found. Available lists: {}",
                        params.list_name,
                        available.join(", ")
                    )));
                }
            }
            Err(e) => {
                return Err(mcp_invalid(format!("Error listing calendars: {e}")));
            }
        };

        let due_date = match params
            .due_date
            .as_deref()
            .map(parse_datetime_or_time)
            .transpose()
        {
            Ok(v) => v,
            Err(e) => return Err(mcp_invalid(format!("Error parsing due_date: {e}"))),
        };
        let start_date = match params.start_date.as_deref().map(parse_datetime).transpose() {
            Ok(v) => v,
            Err(e) => return Err(mcp_invalid(format!("Error parsing start_date: {e}"))),
        };

        let priority = params.priority.as_ref().map(Priority::to_usize);

        // Merge tags into notes if provided
        let notes = if let Some(tags) = &params.tags {
            Some(apply_tags(params.notes.as_deref(), tags))
        } else {
            params.notes.clone()
        };

        match manager.create_reminder(
            &params.title,
            notes.as_deref(),
            Some(&calendar_title),
            priority,
            due_date,
            start_date,
        ) {
            Ok(reminder) => {
                let id = reminder.identifier.clone();
                if let Some(url) = &params.url {
                    let _ = manager.set_url(&id, Some(url));
                }
                if let Some(alarms) = &params.alarms {
                    apply_alarms_reminder(&manager, &id, alarms);
                }
                if let Some(recurrence) = &params.recurrence
                    && let Ok(rule) = parse_recurrence_param(recurrence)
                {
                    let _ = manager.set_recurrence_rule(&id, &rule);
                }
                let updated = manager.get_reminder(&id).unwrap_or(reminder);
                Ok(Json(ReminderOutput::from_item(&updated, &manager)))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "Update an existing reminder. All fields are optional. Can update alarms, recurrence, and URL inline."
    )]
    async fn update_reminder(
        &self,
        Parameters(params): Parameters<UpdateReminderRequest>,
    ) -> Result<Json<ReminderOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        // Parse due date: Some("") means clear, Some(date) means set, None means no change
        let due_date = match &params.due_date {
            Some(due_str) if due_str.is_empty() => Some(None),
            Some(due_str) => match parse_datetime_or_time(due_str) {
                Ok(dt) => Some(Some(dt)),
                Err(e) => return Err(mcp_invalid(format!("Error parsing due_date: {e}"))),
            },
            None => None,
        };

        let start_date = match &params.start_date {
            Some(start_str) if start_str.is_empty() => Some(None),
            Some(start_str) => match parse_datetime(start_str) {
                Ok(dt) => Some(Some(dt)),
                Err(e) => return Err(mcp_invalid(format!("Error parsing start_date: {e}"))),
            },
            None => None,
        };

        if let Some(ref list_name) = params.list_name {
            match manager.list_calendars() {
                Ok(lists) => {
                    if !lists.iter().any(|c| &c.title == list_name) {
                        let available: Vec<_> = lists.iter().map(|c| c.title.as_str()).collect();
                        return Err(mcp_invalid(format!(
                            "List '{}' not found. Available lists: {}",
                            list_name,
                            available.join(", ")
                        )));
                    }
                }
                Err(e) => return Err(mcp_invalid(format!("Error: {e}"))),
            }
        }

        let priority = params.priority.as_ref().map(Priority::to_usize);

        // Merge tags into notes if provided
        let notes = if let Some(tags) = &params.tags {
            // For update, we need the existing notes to merge with
            let existing_notes = manager
                .get_reminder(&params.reminder_id)
                .ok()
                .and_then(|r| r.notes);
            let base = params.notes.as_deref().or(existing_notes.as_deref());
            Some(apply_tags(base, tags))
        } else {
            params.notes.clone()
        };

        match manager.update_reminder(
            &params.reminder_id,
            params.title.as_deref(),
            notes.as_deref(),
            params.completed,
            priority,
            due_date,
            start_date,
            params.list_name.as_deref(),
        ) {
            Ok(reminder) => {
                let id = reminder.identifier.clone();
                if let Some(url) = &params.url {
                    let url_val = if url.is_empty() {
                        None
                    } else {
                        Some(url.as_str())
                    };
                    let _ = manager.set_url(&id, url_val);
                }
                if let Some(alarms) = &params.alarms {
                    apply_alarms_reminder(&manager, &id, alarms);
                }
                if let Some(recurrence) = &params.recurrence {
                    if recurrence.frequency.is_empty() {
                        let _ = manager.remove_recurrence_rules(&id);
                    } else if let Ok(rule) = parse_recurrence_param(recurrence) {
                        let _ = manager.set_recurrence_rule(&id, &rule);
                    }
                }
                let updated = manager.get_reminder(&id).unwrap_or(reminder);
                Ok(Json(ReminderOutput::from_item(&updated, &manager)))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Create a new reminder list (calendar for reminders).")]
    async fn create_reminder_list(
        &self,
        Parameters(params): Parameters<CreateReminderListRequest>,
    ) -> Result<Json<CalendarOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.create_calendar(&params.name) {
            Ok(cal) => Ok(Json(CalendarOutput::from_info(&cal))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Update a reminder list — change name and/or color.")]
    async fn update_reminder_list(
        &self,
        Parameters(params): Parameters<UpdateReminderListRequest>,
    ) -> Result<Json<CalendarOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        let color_rgba = params.color.as_ref().map(CalendarColor::to_rgba);
        match manager.update_calendar(&params.list_id, params.name.as_deref(), color_rgba) {
            Ok(cal) => Ok(Json(CalendarOutput::from_info(&cal))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "Delete a reminder list. WARNING: This will delete all reminders in the list!"
    )]
    async fn delete_reminder_list(
        &self,
        Parameters(params): Parameters<DeleteReminderListRequest>,
    ) -> Result<Json<DeletedResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.delete_calendar(&params.list_id) {
            Ok(_) => Ok(Json(DeletedResponse { id: params.list_id })),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Mark a reminder as completed.")]
    async fn complete_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<Json<ReminderOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.complete_reminder(&params.reminder_id) {
            Ok(_) => {
                let r = manager.get_reminder(&params.reminder_id);
                match r {
                    Ok(r) => Ok(Json(ReminderOutput::from_item(&r, &manager))),
                    Err(e) => Err(mcp_err(&e)),
                }
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Mark a reminder as not completed (uncomplete it).")]
    async fn uncomplete_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<Json<ReminderOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.uncomplete_reminder(&params.reminder_id) {
            Ok(_) => {
                let r = manager.get_reminder(&params.reminder_id);
                match r {
                    Ok(r) => Ok(Json(ReminderOutput::from_item(&r, &manager))),
                    Err(e) => Err(mcp_err(&e)),
                }
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Get a single reminder by its unique identifier.")]
    async fn get_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<Json<ReminderOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.get_reminder(&params.reminder_id) {
            Ok(r) => Ok(Json(ReminderOutput::from_item(&r, &manager))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Delete a reminder from macOS Reminders.")]
    async fn delete_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<Json<DeletedResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.delete_reminder(&params.reminder_id) {
            Ok(_) => Ok(Json(DeletedResponse {
                id: params.reminder_id,
            })),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    // ========================================================================
    // Calendar/Events Tools
    // ========================================================================

    #[tool(description = "List all calendars available in macOS Calendar app.")]
    async fn list_calendars(&self) -> Result<Json<ListResponse<CalendarOutput>>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.list_calendars() {
            Ok(cals) => {
                let items: Vec<_> = cals.iter().map(CalendarOutput::from_info).collect();
                Ok(Json(ListResponse {
                    count: items.len(),
                    items,
                }))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "List calendar events. By default shows today's events. Can specify a date range."
    )]
    async fn list_events(
        &self,
        Parameters(params): Parameters<ListEventsRequest>,
    ) -> Result<Json<ListResponse<EventOutput>>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();

        let events = if params.days == 1 {
            manager.fetch_today_events()
        } else {
            let start = Local::now();
            let end = start + Duration::days(params.days);
            manager.fetch_events(start, end, None)
        };

        match events {
            Ok(items) => {
                let filtered: Vec<_> = if let Some(ref cal_id) = params.calendar_id {
                    items
                        .into_iter()
                        .filter(|e| e.calendar_id.as_deref() == Some(cal_id.as_str()))
                        .collect()
                } else {
                    items
                };
                let items: Vec<_> = filtered
                    .iter()
                    .map(EventOutput::from_item_summary)
                    .collect();
                Ok(Json(ListResponse {
                    count: items.len(),
                    items,
                }))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "Create a new calendar event in macOS Calendar. Can include alarms, recurrence, and URL inline."
    )]
    async fn create_event(
        &self,
        Parameters(params): Parameters<CreateEventRequest>,
    ) -> Result<Json<EventOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();

        let start = match parse_datetime(&params.start) {
            Ok(dt) => dt,
            Err(e) => return Err(mcp_invalid(format!("Error: {e}"))),
        };

        let end = if let Some(end_str) = &params.end {
            match parse_datetime(end_str) {
                Ok(dt) => dt,
                Err(e) => return Err(mcp_invalid(format!("Error: {e}"))),
            }
        } else {
            start + Duration::minutes(params.duration_minutes)
        };

        let calendar_id = if let Some(cal_name) = &params.calendar_name {
            match manager.list_calendars() {
                Ok(calendars) => calendars
                    .iter()
                    .find(|c| &c.title == cal_name)
                    .map(|c| c.identifier.clone()),
                Err(_) => None,
            }
        } else {
            None
        };

        match manager.create_event(
            &params.title,
            start,
            end,
            params.notes.as_deref(),
            params.location.as_deref(),
            calendar_id.as_deref(),
            params.all_day,
        ) {
            Ok(event) => {
                let id = event.identifier.clone();
                if let Some(url) = &params.url {
                    let _ = manager.set_event_url(&id, Some(url));
                }
                if let Some(alarms) = &params.alarms {
                    apply_alarms_event(&manager, &id, alarms);
                }
                if let Some(recurrence) = &params.recurrence
                    && let Ok(rule) = parse_recurrence_param(recurrence)
                {
                    let _ = manager.set_event_recurrence_rule(&id, &rule);
                }
                let updated = manager.get_event(&id).unwrap_or(event);
                Ok(Json(EventOutput::from_item(&updated, &manager)))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Delete a calendar event from macOS Calendar.")]
    async fn delete_event(
        &self,
        Parameters(params): Parameters<EventIdRequest>,
    ) -> Result<Json<DeletedResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.delete_event(&params.event_id, params.affect_future) {
            Ok(_) => Ok(Json(DeletedResponse {
                id: params.event_id,
            })),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Get a single calendar event by its unique identifier.")]
    async fn get_event(
        &self,
        Parameters(params): Parameters<EventIdRequest>,
    ) -> Result<Json<EventOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.get_event(&params.event_id) {
            Ok(e) => Ok(Json(EventOutput::from_item(&e, &manager))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    // ========================================================================
    // Event Calendar Management
    // ========================================================================

    #[tool(description = "Create a new calendar for events.")]
    async fn create_event_calendar(
        &self,
        Parameters(params): Parameters<CreateReminderListRequest>,
    ) -> Result<Json<CalendarOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.create_event_calendar(&params.name) {
            Ok(cal) => Ok(Json(CalendarOutput::from_info(&cal))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(description = "Update an event calendar — change name and/or color.")]
    async fn update_event_calendar(
        &self,
        Parameters(params): Parameters<UpdateEventCalendarRequest>,
    ) -> Result<Json<CalendarOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();

        let color_rgba = params.color.as_ref().map(CalendarColor::to_rgba);

        match manager.update_event_calendar(&params.calendar_id, params.name.as_deref(), color_rgba)
        {
            Ok(cal) => Ok(Json(CalendarOutput::from_info(&cal))),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[tool(
        description = "Delete an event calendar. WARNING: This will delete all events in the calendar!"
    )]
    async fn delete_event_calendar(
        &self,
        Parameters(params): Parameters<DeleteReminderListRequest>,
    ) -> Result<Json<DeletedResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.delete_event_calendar(&params.list_id) {
            Ok(()) => Ok(Json(DeletedResponse { id: params.list_id })),
            Err(e) => Err(mcp_err(&e)),
        }
    }

    // ========================================================================
    // Sources
    // ========================================================================

    #[tool(description = "List all available sources (accounts like iCloud, Local, Exchange).")]
    async fn list_sources(&self) -> Result<Json<ListResponse<SourceOutput>>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.list_sources() {
            Ok(sources) => {
                let items: Vec<_> = sources.iter().map(SourceOutput::from_info).collect();
                Ok(Json(ListResponse {
                    count: items.len(),
                    items,
                }))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    // ========================================================================
    // Event Update Tool
    // ========================================================================

    #[tool(
        description = "Update an existing calendar event. All fields are optional. Can update alarms, recurrence, and URL inline."
    )]
    async fn update_event(
        &self,
        Parameters(params): Parameters<UpdateEventRequest>,
    ) -> Result<Json<EventOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();

        let start = match params.start.as_ref().map(|s| parse_datetime(s)).transpose() {
            Ok(v) => v,
            Err(e) => return Err(mcp_invalid(format!("Error: {e}"))),
        };
        let end = match params.end.as_ref().map(|s| parse_datetime(s)).transpose() {
            Ok(v) => v,
            Err(e) => return Err(mcp_invalid(format!("Error: {e}"))),
        };

        match manager.update_event(
            &params.event_id,
            params.title.as_deref(),
            params.notes.as_deref(),
            params.location.as_deref(),
            start,
            end,
        ) {
            Ok(event) => {
                let id = event.identifier.clone();
                if let Some(url) = &params.url {
                    let url_val = if url.is_empty() {
                        None
                    } else {
                        Some(url.as_str())
                    };
                    let _ = manager.set_event_url(&id, url_val);
                }
                if let Some(alarms) = &params.alarms {
                    apply_alarms_event(&manager, &id, alarms);
                }
                if let Some(recurrence) = &params.recurrence {
                    if recurrence.frequency.is_empty() {
                        let _ = manager.remove_event_recurrence_rules(&id);
                    } else if let Ok(rule) = parse_recurrence_param(recurrence) {
                        let _ = manager.set_event_recurrence_rule(&id, &rule);
                    }
                }
                let updated = manager.get_event(&id).unwrap_or(event);
                Ok(Json(EventOutput::from_item(&updated, &manager)))
            }
            Err(e) => Err(mcp_err(&e)),
        }
    }

    #[cfg(feature = "location")]
    #[tool(
        description = "Get the user's current location (latitude, longitude). Requires location permission."
    )]
    async fn get_current_location(&self) -> Result<Json<CoordinateOutput>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = crate::location::LocationManager::new();
        match manager.get_current_location(std::time::Duration::from_secs(10)) {
            Ok(coord) => Ok(Json(CoordinateOutput {
                latitude: coord.latitude,
                longitude: coord.longitude,
            })),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }
    // ========================================================================
    // Search Tools
    // ========================================================================

    #[tool(
        description = "Search reminders or events by text in title or notes (case-insensitive). Specify item_type to filter, or omit to search both."
    )]
    async fn search(
        &self,
        Parameters(params): Parameters<SearchRequest>,
    ) -> Result<Json<SearchResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let query = params.query.to_lowercase();

        let search_reminders = matches!(params.item_type, None | Some(ItemType::Reminder));
        let search_events = matches!(params.item_type, None | Some(ItemType::Event));

        let reminders = if search_reminders {
            let manager = RemindersManager::new();
            let items = if params.include_completed {
                manager.fetch_all_reminders()
            } else {
                manager.fetch_incomplete_reminders()
            };
            items.ok().map(|items| {
                let filtered: Vec<_> = items
                    .into_iter()
                    .filter(|r| {
                        r.title.to_lowercase().contains(&query)
                            || r.notes
                                .as_deref()
                                .is_some_and(|n| n.to_lowercase().contains(&query))
                    })
                    .map(|r| ReminderOutput::from_item_summary(&r))
                    .collect();
                ListResponse {
                    count: filtered.len(),
                    items: filtered,
                }
            })
        } else {
            None
        };

        let events = if search_events {
            let manager = EventsManager::new();
            let start = Local::now();
            let end = start + Duration::days(params.days);
            manager.fetch_events(start, end, None).ok().map(|items| {
                let filtered: Vec<_> = items
                    .into_iter()
                    .filter(|e| {
                        e.title.to_lowercase().contains(&query)
                            || e.notes
                                .as_deref()
                                .is_some_and(|n| n.to_lowercase().contains(&query))
                    })
                    .map(|e| EventOutput::from_item_summary(&e))
                    .collect();
                ListResponse {
                    count: filtered.len(),
                    items: filtered,
                }
            })
        } else {
            None
        };

        Ok(Json(SearchResponse {
            query: params.query,
            reminders,
            events,
        }))
    }

    // ========================================================================
    // Batch Operations
    // ========================================================================

    #[tool(description = "Delete multiple reminders or events at once.")]
    async fn batch_delete(
        &self,
        Parameters(params): Parameters<BatchDeleteRequest>,
    ) -> Result<Json<BatchResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let mut succeeded = 0usize;
        let mut errors = Vec::new();

        match params.item_type {
            ItemType::Reminder => {
                let manager = RemindersManager::new();
                for id in &params.item_ids {
                    match manager.delete_reminder(id) {
                        Ok(_) => succeeded += 1,
                        Err(e) => errors.push(format!("{id}: {e}")),
                    }
                }
            }
            ItemType::Event => {
                let manager = EventsManager::new();
                for id in &params.item_ids {
                    match manager.delete_event(id, params.affect_future) {
                        Ok(_) => succeeded += 1,
                        Err(e) => errors.push(format!("{id}: {e}")),
                    }
                }
            }
        }

        let err_items: Vec<_> = errors
            .into_iter()
            .map(|e| {
                let (id, msg) = e.split_once(": ").unwrap_or(("unknown", &e));
                BatchItemError {
                    item_id: id.to_string(),
                    message: msg.to_string(),
                }
            })
            .collect();
        Ok(Json(BatchResponse {
            total: params.item_ids.len(),
            succeeded,
            failed: err_items.len(),
            errors: err_items,
        }))
    }

    #[tool(description = "Move multiple reminders to a different list at once.")]
    async fn batch_move(
        &self,
        Parameters(params): Parameters<BatchMoveRequest>,
    ) -> Result<Json<BatchResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        let mut succeeded = 0usize;
        let mut errors = Vec::new();

        for id in &params.reminder_ids {
            match manager.update_reminder(
                id,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&params.destination_list_name),
            ) {
                Ok(_) => succeeded += 1,
                Err(e) => errors.push(format!("{id}: {e}")),
            }
        }

        let err_items: Vec<_> = errors
            .into_iter()
            .map(|e| {
                let (id, msg) = e.split_once(": ").unwrap_or(("unknown", &e));
                BatchItemError {
                    item_id: id.to_string(),
                    message: msg.to_string(),
                }
            })
            .collect();
        Ok(Json(BatchResponse {
            total: params.reminder_ids.len(),
            succeeded,
            failed: err_items.len(),
            errors: err_items,
        }))
    }

    #[tool(description = "Update multiple reminders or events at once.")]
    async fn batch_update(
        &self,
        Parameters(params): Parameters<BatchUpdateRequest>,
    ) -> Result<Json<BatchResponse>, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let mut succeeded = 0usize;
        let mut errors = Vec::new();

        match params.item_type {
            ItemType::Reminder => {
                let manager = RemindersManager::new();
                for item in &params.updates {
                    let priority = item.priority.as_ref().map(Priority::to_usize);
                    let due_date = match &item.due_date {
                        Some(s) if s.is_empty() => Some(None),
                        Some(s) => match parse_datetime_or_time(s) {
                            Ok(dt) => Some(Some(dt)),
                            Err(e) => {
                                errors.push(format!("{}: {e}", item.item_id));
                                continue;
                            }
                        },
                        None => None,
                    };
                    match manager.update_reminder(
                        &item.item_id,
                        item.title.as_deref(),
                        item.notes.as_deref(),
                        item.completed,
                        priority,
                        due_date,
                        None,
                        None,
                    ) {
                        Ok(_) => succeeded += 1,
                        Err(e) => errors.push(format!("{}: {e}", item.item_id)),
                    }
                }
            }
            ItemType::Event => {
                let manager = EventsManager::new();
                for item in &params.updates {
                    match manager.update_event(
                        &item.item_id,
                        item.title.as_deref(),
                        item.notes.as_deref(),
                        None,
                        None,
                        None,
                    ) {
                        Ok(_) => succeeded += 1,
                        Err(e) => errors.push(format!("{}: {e}", item.item_id)),
                    }
                }
            }
        }

        let total = params.updates.len();
        let err_items: Vec<_> = errors
            .into_iter()
            .map(|e| {
                let (id, msg) = e.split_once(": ").unwrap_or(("unknown", &e));
                BatchItemError {
                    item_id: id.to_string(),
                    message: msg.to_string(),
                }
            })
            .collect();
        Ok(Json(BatchResponse {
            total,
            succeeded,
            failed: err_items.len(),
            errors: err_items,
        }))
    }
}

/// Parse a RecurrenceParam into a RecurrenceRule.
fn parse_recurrence_param(
    params: &RecurrenceParam,
) -> std::result::Result<crate::RecurrenceRule, String> {
    let frequency = match params.frequency.as_str() {
        "daily" => crate::RecurrenceFrequency::Daily,
        "weekly" => crate::RecurrenceFrequency::Weekly,
        "monthly" => crate::RecurrenceFrequency::Monthly,
        "yearly" => crate::RecurrenceFrequency::Yearly,
        other => {
            return Err(format!(
                "Invalid frequency: '{}'. Use daily, weekly, monthly, or yearly.",
                other
            ));
        }
    };

    let end = if let Some(count) = params.end_after_count {
        crate::RecurrenceEndCondition::AfterCount(count)
    } else if let Some(date_str) = &params.end_date {
        let dt = parse_datetime(date_str)?;
        crate::RecurrenceEndCondition::OnDate(dt)
    } else {
        crate::RecurrenceEndCondition::Never
    };

    Ok(crate::RecurrenceRule {
        frequency,
        interval: params.interval,
        end,
        days_of_week: params.days_of_week.clone(),
        days_of_month: params.days_of_month.clone(),
    })
}

/// Parse a date/time string, defaulting to today if only time is given.
fn parse_datetime_or_time(s: &str) -> Result<DateTime<Local>, String> {
    // Try full datetime or date first
    if let Ok(dt) = parse_datetime(s) {
        return Ok(dt);
    }
    // Try time-only: "HH:MM" → use today's date
    if let Ok(time) = chrono::NaiveTime::parse_from_str(s, "%H:%M") {
        let today = Local::now().date_naive();
        let dt = today.and_time(time);
        return Local
            .from_local_datetime(&dt)
            .single()
            .ok_or_else(|| "Invalid local datetime".to_string());
    }
    Err(
        "Invalid date format. Use 'YYYY-MM-DD', 'YYYY-MM-DD HH:MM', or 'HH:MM' (uses today)"
            .to_string(),
    )
}

/// Apply alarms to a reminder, clearing existing ones first.
fn apply_alarms_reminder(manager: &RemindersManager, id: &str, alarms: &[AlarmParam]) {
    // Clear existing alarms
    if let Ok(existing) = manager.get_alarms(id) {
        for i in (0..existing.len()).rev() {
            let _ = manager.remove_alarm(id, i);
        }
    }
    // Add new alarms
    for param in alarms {
        let alarm = alarm_param_to_info(param);
        let _ = manager.add_alarm(id, &alarm);
    }
}

/// Apply alarms to an event, clearing existing ones first.
fn apply_alarms_event(manager: &EventsManager, id: &str, alarms: &[AlarmParam]) {
    if let Ok(existing) = manager.get_event_alarms(id) {
        for i in (0..existing.len()).rev() {
            let _ = manager.remove_event_alarm(id, i);
        }
    }
    for param in alarms {
        let alarm = alarm_param_to_info(param);
        let _ = manager.add_event_alarm(id, &alarm);
    }
}

/// Convert an AlarmParam to an AlarmInfo.
fn alarm_param_to_info(param: &AlarmParam) -> crate::AlarmInfo {
    let proximity = match param.proximity.as_deref() {
        Some("enter") => crate::AlarmProximity::Enter,
        Some("leave") => crate::AlarmProximity::Leave,
        _ => crate::AlarmProximity::None,
    };
    let location = if let (Some(title), Some(lat), Some(lng)) =
        (&param.location_title, param.latitude, param.longitude)
    {
        Some(crate::StructuredLocation {
            title: title.clone(),
            latitude: lat,
            longitude: lng,
            radius: param.radius.unwrap_or(100.0),
        })
    } else {
        None
    };
    crate::AlarmInfo {
        relative_offset: param.relative_offset,
        absolute_date: None,
        proximity,
        location,
    }
}

/// Format alarms for display output.
/// Extract #tags from notes content.
fn extract_tags(notes: &str) -> Vec<String> {
    notes
        .split_whitespace()
        .filter(|w| w.starts_with('#') && w.len() > 1)
        .map(|w| w[1..].to_string())
        .collect()
}

/// Merge tags into notes. Removes existing #tag tokens, appends new ones.
fn apply_tags(notes: Option<&str>, tags: &[String]) -> String {
    // Keep lines that aren't purely tags
    let mut result: Vec<String> = notes
        .unwrap_or("")
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            // Remove lines that are only #tags
            !trimmed
                .split_whitespace()
                .all(|w| w.starts_with('#') && w.len() > 1)
                || trimmed.is_empty()
        })
        .map(String::from)
        .collect();
    // Remove trailing empty lines
    while result.last().is_some_and(std::string::String::is_empty) {
        result.pop();
    }
    if !tags.is_empty() {
        if !result.is_empty() {
            result.push(String::new());
        }
        result.push(
            tags.iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    result.join("\n")
}

// ============================================================================
// Prompts
// ============================================================================

#[prompt_router]
impl EventKitServer {
    /// List all incomplete (not yet finished) reminders, optionally filtered by list name.
    #[prompt(
        name = "incomplete_reminders",
        description = "List all incomplete reminders"
    )]
    async fn incomplete_reminders(
        &self,
        Parameters(args): Parameters<ListRemindersPromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        let reminders = manager.fetch_incomplete_reminders().map_err(|e| {
            McpError::internal_error(format!("Failed to list reminders: {e}"), None)
        })?;

        // Filter by list name if provided
        let reminders: Vec<_> = if let Some(ref name) = args.list_name {
            reminders
                .into_iter()
                .filter(|r| r.calendar_title.as_deref() == Some(name.as_str()))
                .collect()
        } else {
            reminders
        };

        let mut output = String::new();
        for r in &reminders {
            output.push_str(&format!(
                "- [{}] {} (id: {}){}{}\n",
                if r.completed { "x" } else { " " },
                r.title,
                r.identifier,
                r.due_date
                    .map(|d| format!(", due: {}", d.format("%Y-%m-%d %H:%M")))
                    .unwrap_or_default(),
                r.calendar_title
                    .as_ref()
                    .map(|l| format!(", list: {l}"))
                    .unwrap_or_default(),
            ));
        }

        if output.is_empty() {
            output = "No incomplete reminders found.".to_string();
        }

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!(
                "Here are the current incomplete reminders:\n\n{output}\n\nPlease help me manage these reminders."
            ),
        )])
        .with_description("Incomplete reminders"))
    }

    /// List all reminder lists (calendars) available in macOS Reminders.
    #[prompt(
        name = "reminder_lists",
        description = "List all reminder lists available in Reminders"
    )]
    async fn reminder_lists_prompt(&self) -> Result<GetPromptResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        let lists = manager.list_calendars().map_err(|e| {
            McpError::internal_error(format!("Failed to list calendars: {e}"), None)
        })?;

        let mut output = String::new();
        for list in &lists {
            output.push_str(&format!("- {} (id: {})\n", list.title, list.identifier));
        }

        if output.is_empty() {
            output = "No reminder lists found.".to_string();
        }

        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            PromptMessageRole::User,
            format!(
                "Here are the available reminder lists:\n\n{output}\n\nWhich list would you like to work with?"
            ),
        )])
        .with_description("Available reminder lists"))
    }

    /// Move a reminder to a different reminder list.
    #[prompt(
        name = "move_reminder",
        description = "Move a reminder to a different list"
    )]
    async fn move_reminder_prompt(
        &self,
        Parameters(args): Parameters<MoveReminderPromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        // Find the destination calendar
        let lists = manager.list_calendars().map_err(|e| {
            McpError::internal_error(format!("Failed to list calendars: {e}"), None)
        })?;

        let dest = lists.iter().find(|l| {
            l.title
                .to_lowercase()
                .contains(&args.destination_list.to_lowercase())
        });

        match dest {
            Some(dest_list) => {
                match manager.update_reminder(
                    &args.reminder_id,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&dest_list.title),
                ) {
                    Ok(updated) => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                        PromptMessageRole::User,
                        format!(
                            "Moved reminder \"{}\" to list \"{}\".",
                            updated.title, dest_list.title
                        ),
                    )])
                    .with_description("Reminder moved")),
                    Err(e) => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                        PromptMessageRole::User,
                        format!("Failed to move reminder: {e}"),
                    )])
                    .with_description("Move failed")),
                }
            }
            None => {
                let available: Vec<&str> = lists.iter().map(|l| l.title.as_str()).collect();
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!(
                        "Could not find reminder list \"{}\". Available lists: {}",
                        args.destination_list,
                        available.join(", ")
                    ),
                )])
                .with_description("List not found"))
            }
        }
    }

    /// Create a new reminder with optional notes, priority, due date, and list.
    #[prompt(
        name = "create_detailed_reminder",
        description = "Create a reminder with detailed context like notes, priority, and due date"
    )]
    async fn create_detailed_reminder_prompt(
        &self,
        Parameters(args): Parameters<CreateReminderPromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        let due = args
            .due_date
            .as_deref()
            .map(parse_datetime)
            .transpose()
            .map_err(|e| McpError::internal_error(format!("Invalid due date: {e}"), None))?;

        match manager.create_reminder(
            &args.title,
            args.notes.as_deref(),
            args.list_name.as_deref(),
            args.priority.map(|p| p as usize),
            due,
            None,
        ) {
            Ok(reminder) => {
                let mut details = format!("Created reminder: \"{}\"", reminder.title);
                if let Some(notes) = &reminder.notes {
                    details.push_str(&format!("\nNotes: {notes}"));
                }
                if reminder.priority > 0 {
                    details.push_str(&format!("\nPriority: {}", reminder.priority));
                }
                if let Some(due) = &reminder.due_date {
                    details.push_str(&format!("\nDue: {}", due.format("%Y-%m-%d %H:%M")));
                }
                if let Some(list) = &reminder.calendar_title {
                    details.push_str(&format!("\nList: {list}"));
                }

                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    details,
                )])
                .with_description("Reminder created"))
            }
            Err(e) => Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                PromptMessageRole::User,
                format!("Failed to create reminder: {e}"),
            )])
            .with_description("Creation failed")),
        }
    }
}

// Implement the server handler
#[tool_handler]
#[prompt_handler]
impl rmcp::ServerHandler for EventKitServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_instructions(
            "This MCP server provides access to macOS Calendar events and Reminders. \
             Use the available tools to list, create, update, and delete calendar events \
             and reminders. Authorization is handled automatically on first use.",
        )
    }
}

/// Serve the EventKit MCP server on any async read/write transport.
///
/// Used by the in-process gateway (via `DuplexStream`) and for testing.
/// The standalone binary uses [`run_mcp_server`] which wraps this with stdio.
pub async fn serve_on<T>(transport: T) -> anyhow::Result<()>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let server = EventKitServer::new();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}

/// Run the EventKit MCP server on stdio transport.
///
/// This initializes logging to stderr (MCP uses stdout/stdin for protocol)
/// and starts the MCP server. Used by the standalone binary (`eventkit --mcp`).
pub async fn run_mcp_server() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let server = EventKitServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// ============================================================================
// Dump helpers — serialize objects to JSON for CLI debugging
// ============================================================================

/// Dump a single reminder as pretty JSON (with alarms, recurrence, tags).
pub fn dump_reminder(id: &str) -> Result<String, crate::EventKitError> {
    let manager = RemindersManager::new();
    let r = manager.get_reminder(id)?;
    let output = ReminderOutput::from_item(&r, &manager);
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump all reminders as pretty JSON (summary mode — no alarm/recurrence fetch).
pub fn dump_reminders(list_name: Option<&str>) -> Result<String, crate::EventKitError> {
    let manager = RemindersManager::new();
    let items = manager.fetch_all_reminders()?;
    let filtered: Vec<_> = if let Some(name) = list_name {
        items
            .into_iter()
            .filter(|r| r.calendar_title.as_deref() == Some(name))
            .collect()
    } else {
        items
    };
    let output: Vec<_> = filtered
        .iter()
        .map(ReminderOutput::from_item_summary)
        .collect();
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump a single event as pretty JSON (with alarms, recurrence, attendees).
pub fn dump_event(id: &str) -> Result<String, crate::EventKitError> {
    let manager = EventsManager::new();
    let e = manager.get_event(id)?;
    let output = EventOutput::from_item(&e, &manager);
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump upcoming events as pretty JSON.
pub fn dump_events(days: i64) -> Result<String, crate::EventKitError> {
    let manager = EventsManager::new();
    let items = manager.fetch_upcoming_events(days)?;
    let output: Vec<_> = items.iter().map(EventOutput::from_item_summary).collect();
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump all reminder lists as pretty JSON.
pub fn dump_reminder_lists() -> Result<String, crate::EventKitError> {
    let manager = RemindersManager::new();
    let lists = manager.list_calendars()?;
    let output: Vec<_> = lists.iter().map(CalendarOutput::from_info).collect();
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump all event calendars as pretty JSON.
pub fn dump_calendars() -> Result<String, crate::EventKitError> {
    let manager = EventsManager::new();
    let cals = manager.list_calendars()?;
    let output: Vec<_> = cals.iter().map(CalendarOutput::from_info).collect();
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

/// Dump all sources as pretty JSON.
pub fn dump_sources() -> Result<String, crate::EventKitError> {
    let manager = RemindersManager::new();
    let sources = manager.list_sources()?;
    let output: Vec<_> = sources.iter().map(SourceOutput::from_info).collect();
    Ok(serde_json::to_string_pretty(&output).unwrap())
}

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

// ============================================================================
// Request/Response Types for Tools
// ============================================================================

/// Empty request for tools that don't require any input parameters.
/// Having an explicit empty schema helps MCP clients understand no input is needed.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "No input parameters required")]
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
    /// Priority level: 1 (high), 5 (medium), 9 (low), or 0 (none)
    pub priority: Option<usize>,
    /// Optional due date in format 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'
    pub due_date: Option<String>,
    /// Optional start date when to begin working (format: 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM')
    pub start_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateReminderRequest {
    /// The unique identifier of the reminder to update
    pub reminder_id: String,
    /// The name of the reminder list to move this reminder to (use list_reminder_lists to see available lists)
    pub list_name: Option<String>,
    /// New title for the reminder
    pub title: Option<String>,
    /// New notes for the reminder
    pub notes: Option<String>,
    /// Mark as completed (true) or incomplete (false)
    pub completed: Option<bool>,
    /// New priority level: 1 (high), 5 (medium), 9 (low), or 0 (none)
    pub priority: Option<usize>,
    /// New due date in format 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'. Set to empty string to clear.
    pub due_date: Option<String>,
    /// New start date (format: 'YYYY-MM-DD' or 'YYYY-MM-DD HH:MM'). Set to empty string to clear.
    pub start_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateReminderListRequest {
    /// The name of the new reminder list to create
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RenameReminderListRequest {
    /// The unique identifier of the reminder list to rename
    pub list_id: String,
    /// The new name for the reminder list
    pub new_name: String,
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
    /// Optional: Filter to a specific calendar by name
    pub calendar_name: Option<String>,
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
}

fn default_duration() -> i64 {
    60
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EventIdRequest {
    /// The unique identifier of the event
    pub event_id: String,
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
    async fn list_reminder_lists(&self) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.list_calendars() {
            Ok(lists) => {
                if lists.is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "No reminder lists found.",
                    )]));
                }
                let mut output = String::from("Reminder Lists:\n");
                for (i, list) in lists.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} (id: {})\n",
                        i + 1,
                        list.title,
                        list.identifier
                    ));
                }
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "List reminders from macOS Reminders app. Can filter by completion status."
    )]
    async fn list_reminders(
        &self,
        Parameters(params): Parameters<ListRemindersRequest>,
    ) -> Result<CallToolResult, McpError> {
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

                if filtered.is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "No reminders found.",
                    )]));
                }

                let mut output = String::from("Reminders:\n");
                for reminder in &filtered {
                    let status = if reminder.completed { "x" } else { " " };
                    let priority = match reminder.priority {
                        1 => " [HIGH]",
                        5 => " [MED]",
                        9 => " [LOW]",
                        _ => "",
                    };
                    let list = reminder.calendar_title.as_deref().unwrap_or("Unknown List");

                    output.push_str(&format!(
                        "[{}] {}{}\n    List: {} | ID: {}\n",
                        status, reminder.title, priority, list, reminder.identifier
                    ));
                    if let Some(due) = reminder.due_date {
                        output.push_str(&format!("    Due: {}\n", due.format("%Y-%m-%d %H:%M")));
                    }
                    if let Some(start) = reminder.start_date {
                        output
                            .push_str(&format!("    Start: {}\n", start.format("%Y-%m-%d %H:%M")));
                    }
                    if let Some(notes) = &reminder.notes
                        && !notes.is_empty()
                    {
                        output.push_str(&format!("    Notes: {}\n", notes));
                    }
                }
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Create a new reminder in macOS Reminders. You MUST specify which list to add it to. Use list_reminder_lists first to see available lists."
    )]
    async fn create_reminder(
        &self,
        Parameters(params): Parameters<CreateReminderRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        // Validate the list exists - list_name is now required
        let calendar_title = match manager.list_calendars() {
            Ok(lists) => {
                if let Some(cal) = lists.iter().find(|c| c.title == params.list_name) {
                    cal.title.clone()
                } else {
                    let available: Vec<_> = lists.iter().map(|c| c.title.as_str()).collect();
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "List '{}' not found. Available lists: {}",
                        params.list_name,
                        available.join(", ")
                    ))]));
                }
            }
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error listing calendars: {}",
                    e
                ))]));
            }
        };

        // Parse due date if provided
        let due_date = if let Some(due_str) = &params.due_date {
            match parse_datetime(due_str) {
                Ok(dt) => Some(dt),
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error parsing due_date: {}",
                        e
                    ))]));
                }
            }
        } else {
            None
        };

        // Parse start date if provided
        let start_date = if let Some(start_str) = &params.start_date {
            match parse_datetime(start_str) {
                Ok(dt) => Some(dt),
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error parsing start_date: {}",
                        e
                    ))]));
                }
            }
        } else {
            None
        };

        match manager.create_reminder(
            &params.title,
            params.notes.as_deref(),
            Some(&calendar_title),
            params.priority,
            due_date,
            start_date,
        ) {
            Ok(reminder) => {
                let mut msg = format!(
                    "Created reminder: {} (id: {})\nList: {}",
                    reminder.title, reminder.identifier, params.list_name
                );
                if let Some(due) = reminder.due_date {
                    msg.push_str(&format!("\nDue: {}", due.format("%Y-%m-%d %H:%M")));
                }
                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Update an existing reminder. All fields are optional.")]
    async fn update_reminder(
        &self,
        Parameters(params): Parameters<UpdateReminderRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();

        // Parse due date: Some("") means clear, Some(date) means set, None means no change
        let due_date = match &params.due_date {
            Some(due_str) if due_str.is_empty() => Some(None), // Clear
            Some(due_str) => match parse_datetime(due_str) {
                Ok(dt) => Some(Some(dt)), // Set
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error parsing due_date: {}",
                        e
                    ))]));
                }
            },
            None => None, // No change
        };

        // Parse start date similarly
        let start_date = match &params.start_date {
            Some(start_str) if start_str.is_empty() => Some(None), // Clear
            Some(start_str) => match parse_datetime(start_str) {
                Ok(dt) => Some(Some(dt)), // Set
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error parsing start_date: {}",
                        e
                    ))]));
                }
            },
            None => None, // No change
        };

        // Validate the target list exists if specified
        if let Some(ref list_name) = params.list_name {
            match manager.list_calendars() {
                Ok(lists) => {
                    if !lists.iter().any(|c| &c.title == list_name) {
                        let available: Vec<_> = lists.iter().map(|c| c.title.as_str()).collect();
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "List '{}' not found. Available lists: {}",
                            list_name,
                            available.join(", ")
                        ))]));
                    }
                }
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Error listing calendars: {}",
                        e
                    ))]));
                }
            }
        }

        match manager.update_reminder(
            &params.reminder_id,
            params.title.as_deref(),
            params.notes.as_deref(),
            params.completed,
            params.priority,
            due_date,
            start_date,
            params.list_name.as_deref(),
        ) {
            Ok(reminder) => {
                let mut msg = format!("Updated reminder: {}", reminder.title);
                if reminder.completed {
                    msg.push_str(" [Completed]");
                }
                if let Some(list) = &reminder.calendar_title {
                    msg.push_str(&format!("\nList: {}", list));
                }
                if let Some(due) = reminder.due_date {
                    msg.push_str(&format!("\nDue: {}", due.format("%Y-%m-%d %H:%M")));
                }
                Ok(CallToolResult::success(vec![Content::text(msg)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Create a new reminder list (calendar for reminders).")]
    async fn create_reminder_list(
        &self,
        Parameters(params): Parameters<CreateReminderListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.create_calendar(&params.name) {
            Ok(calendar) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Created reminder list: {} (id: {})",
                calendar.title, calendar.identifier
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Rename an existing reminder list.")]
    async fn rename_reminder_list(
        &self,
        Parameters(params): Parameters<RenameReminderListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.rename_calendar(&params.list_id, &params.new_name) {
            Ok(calendar) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Renamed reminder list to: {}",
                calendar.title
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "Delete a reminder list. WARNING: This will delete all reminders in the list!"
    )]
    async fn delete_reminder_list(
        &self,
        Parameters(params): Parameters<DeleteReminderListRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.delete_calendar(&params.list_id) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Deleted reminder list {}.",
                params.list_id
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Mark a reminder as completed.")]
    async fn complete_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.complete_reminder(&params.reminder_id) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Marked reminder {} as completed.",
                params.reminder_id
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Delete a reminder from macOS Reminders.")]
    async fn delete_reminder(
        &self,
        Parameters(params): Parameters<ReminderIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        match manager.delete_reminder(&params.reminder_id) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Deleted reminder {}.",
                params.reminder_id
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    // ========================================================================
    // Calendar/Events Tools
    // ========================================================================

    #[tool(description = "List all calendars available in macOS Calendar app.")]
    async fn list_calendars(&self) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.list_calendars() {
            Ok(calendars) => {
                if calendars.is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "No calendars found.",
                    )]));
                }
                let mut output = String::from("Calendars:\n");
                for (i, cal) in calendars.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} (id: {})\n",
                        i + 1,
                        cal.title,
                        cal.identifier
                    ));
                }
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(
        description = "List calendar events. By default shows today's events. Can specify a date range."
    )]
    async fn list_events(
        &self,
        Parameters(params): Parameters<ListEventsRequest>,
    ) -> Result<CallToolResult, McpError> {
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
                let filtered: Vec<_> = if let Some(name) = params.calendar_name {
                    items
                        .into_iter()
                        .filter(|e| e.calendar_title.as_deref() == Some(&name))
                        .collect()
                } else {
                    items
                };

                if filtered.is_empty() {
                    return Ok(CallToolResult::success(vec![Content::text(
                        "No events found.",
                    )]));
                }

                let mut output = String::from("Calendar Events:\n");
                for event in &filtered {
                    let calendar = event.calendar_title.as_deref().unwrap_or("Unknown");
                    let all_day = if event.all_day { " [All Day]" } else { "" };

                    output.push_str(&format!(
                        "- {}{}\n  Start: {} | End: {}\n  Calendar: {} | ID: {}\n",
                        event.title,
                        all_day,
                        event.start_date,
                        event.end_date,
                        calendar,
                        event.identifier
                    ));

                    if let Some(location) = &event.location
                        && !location.is_empty()
                    {
                        output.push_str(&format!("  Location: {}\n", location));
                    }
                    if let Some(notes) = &event.notes
                        && !notes.is_empty()
                    {
                        output.push_str(&format!("  Notes: {}\n", notes));
                    }
                }
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Create a new calendar event in macOS Calendar.")]
    async fn create_event(
        &self,
        Parameters(params): Parameters<CreateEventRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();

        let start = match parse_datetime(&params.start) {
            Ok(dt) => dt,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "Error: {}",
                    e
                ))]));
            }
        };

        let end = if let Some(end_str) = &params.end {
            match parse_datetime(end_str) {
                Ok(dt) => dt,
                Err(e) => {
                    return Ok(CallToolResult::success(vec![Content::text(format!(
                        "Error: {}",
                        e
                    ))]));
                }
            }
        } else {
            start + Duration::minutes(params.duration_minutes)
        };

        // Find calendar by name if specified
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
            Ok(event) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Created event: {} (id: {})\nStart: {} | End: {}",
                event.title, event.identifier, event.start_date, event.end_date
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Delete a calendar event from macOS Calendar.")]
    async fn delete_event(
        &self,
        Parameters(params): Parameters<EventIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = EventsManager::new();
        match manager.delete_event(&params.event_id) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Deleted event {}.",
                params.event_id
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }
}

// ============================================================================
// Prompts
// ============================================================================

#[prompt_router]
impl EventKitServer {
    /// List all incomplete (not yet finished) reminders, optionally filtered by list name.
    #[prompt(name = "incomplete_reminders", description = "List all incomplete reminders")]
    async fn incomplete_reminders(
        &self,
        Parameters(args): Parameters<ListRemindersPromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let _permit = self.concurrency.acquire().await.unwrap();
        let manager = RemindersManager::new();
        let reminders = manager
            .fetch_incomplete_reminders()
            .map_err(|e| McpError::internal_error(format!("Failed to list reminders: {e}"), None))?;

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
        let lists = manager
            .list_calendars()
            .map_err(|e| McpError::internal_error(format!("Failed to list calendars: {e}"), None))?;

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
        let lists = manager
            .list_calendars()
            .map_err(|e| McpError::internal_error(format!("Failed to list calendars: {e}"), None))?;

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
                    Ok(updated) => Ok(GetPromptResult::new(vec![
                        PromptMessage::new_text(
                            PromptMessageRole::User,
                            format!(
                                "Moved reminder \"{}\" to list \"{}\".",
                                updated.title, dest_list.title
                            ),
                        ),
                    ])
                    .with_description("Reminder moved")),
                    Err(e) => Ok(GetPromptResult::new(vec![
                        PromptMessage::new_text(
                            PromptMessageRole::User,
                            format!("Failed to move reminder: {e}"),
                        ),
                    ])
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

/// Run the EventKit MCP server on stdio transport.
///
/// This initializes logging to stderr (MCP uses stdout/stdin for protocol)
/// and starts the MCP server.
pub async fn run_mcp_server() -> anyhow::Result<()> {
    // Initialize logging to stderr (MCP uses stdout/stdin for protocol)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Create and run the server with STDIO transport
    let server = EventKitServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}

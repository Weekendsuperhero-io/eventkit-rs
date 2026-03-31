use block2::RcBlock;
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike};
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_event_kit::{
    EKAlarm, EKAlarmProximity, EKAuthorizationStatus, EKCalendar, EKCalendarItem, EKEntityType,
    EKEvent, EKEventStore, EKRecurrenceDayOfWeek, EKRecurrenceEnd, EKRecurrenceFrequency,
    EKRecurrenceRule, EKReminder, EKSource, EKSpan, EKStructuredLocation, EKWeekday,
};
use objc2_foundation::{
    NSArray, NSCalendar, NSDate, NSDateComponents, NSError, NSNumber, NSString,
};
use std::sync::{Arc, Condvar, Mutex};
use thiserror::Error;

#[cfg(feature = "location")]
#[path = "location.rs"]
pub mod location;

#[cfg(feature = "mcp")]
#[path = "mcp.rs"]
pub mod mcp;

/// Errors that can occur when working with EventKit
#[derive(Error, Debug)]
pub enum EventKitError {
    #[error("Authorization denied")]
    AuthorizationDenied,

    #[error("Authorization restricted by system policy")]
    AuthorizationRestricted,

    #[error("Authorization not determined")]
    AuthorizationNotDetermined,

    #[error("Failed to request authorization: {0}")]
    AuthorizationRequestFailed(String),

    #[error("No default calendar")]
    NoDefaultCalendar,

    #[error("Calendar not found: {0}")]
    CalendarNotFound(String),

    #[error("Item not found: {0}")]
    ItemNotFound(String),

    #[error("Failed to save: {0}")]
    SaveFailed(String),

    #[error("Failed to delete: {0}")]
    DeleteFailed(String),

    #[error("Failed to fetch: {0}")]
    FetchFailed(String),

    #[error("EventKit error: {0}")]
    EventKitError(String),

    #[error("Invalid date range")]
    InvalidDateRange,
}

/// Backward compatibility alias
pub type RemindersError = EventKitError;

/// Result type for EventKit operations
pub type Result<T> = std::result::Result<T, EventKitError>;

/// Represents a reminder item with its properties
#[derive(Debug, Clone)]
pub struct ReminderItem {
    /// Unique identifier for the reminder
    pub identifier: String,
    /// Title of the reminder
    pub title: String,
    /// Optional notes/description
    pub notes: Option<String>,
    /// Whether the reminder is completed
    pub completed: bool,
    /// Priority (0 = none, 1-4 = high, 5 = medium, 6-9 = low)
    pub priority: usize,
    /// Calendar/list the reminder belongs to
    pub calendar_title: Option<String>,
    /// Calendar/list identifier
    pub calendar_id: Option<String>,
    /// Due date for the reminder
    pub due_date: Option<DateTime<Local>>,
    /// Start date (when to start working on it)
    pub start_date: Option<DateTime<Local>>,
    /// Completion date (when it was completed)
    pub completion_date: Option<DateTime<Local>>,
    /// External identifier for the reminder (server-provided)
    pub external_identifier: Option<String>,
    /// Location associated with the reminder
    pub location: Option<String>,
    /// URL associated with the reminder
    pub url: Option<String>,
    /// Creation date of the reminder
    pub creation_date: Option<DateTime<Local>>,
    /// Last modified date of the reminder
    pub last_modified_date: Option<DateTime<Local>>,
    /// Timezone of the reminder
    pub timezone: Option<String>,
    /// Whether the reminder has alarms
    pub has_alarms: bool,
    /// Whether the reminder has recurrence rules
    pub has_recurrence_rules: bool,
    /// Whether the reminder has attendees
    pub has_attendees: bool,
    /// Whether the reminder has notes
    pub has_notes: bool,
    /// Attendees on this reminder (usually empty, possible on shared lists)
    pub attendees: Vec<ParticipantInfo>,
}

/// Type of calendar/source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarType {
    Local,
    CalDAV,
    Exchange,
    Subscription,
    Birthday,
    Unknown,
}

/// An account source (iCloud, Local, Exchange, etc.)
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub identifier: String,
    pub title: String,
    pub source_type: String,
}

/// Represents a calendar (reminder list or event calendar).
#[derive(Debug, Clone)]
pub struct CalendarInfo {
    /// Unique identifier
    pub identifier: String,
    /// Title of the calendar
    pub title: String,
    /// Source name (e.g., iCloud, Local)
    pub source: Option<String>,
    /// Source identifier
    pub source_id: Option<String>,
    /// Calendar type
    pub calendar_type: CalendarType,
    /// Whether items can be added/modified/deleted
    pub allows_modifications: bool,
    /// Whether the calendar itself can be modified (renamed/deleted)
    pub is_immutable: bool,
    /// Whether this is a URL-subscribed read-only calendar
    pub is_subscribed: bool,
    /// Calendar color as RGBA (0.0-1.0)
    pub color: Option<(f64, f64, f64, f64)>,
    /// Entity types this calendar supports ("event", "reminder")
    pub allowed_entity_types: Vec<String>,
}

/// Proximity trigger for a location-based alarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmProximity {
    /// No proximity trigger.
    None,
    /// Trigger when entering the location.
    Enter,
    /// Trigger when leaving the location.
    Leave,
}

/// A structured location for geofenced alarms.
#[derive(Debug, Clone)]
pub struct StructuredLocation {
    /// Display title for the location.
    pub title: String,
    /// Latitude of the location.
    pub latitude: f64,
    /// Longitude of the location.
    pub longitude: f64,
    /// Geofence radius in meters.
    pub radius: f64,
}

/// An alarm attached to a reminder or event.
#[derive(Debug, Clone)]
pub struct AlarmInfo {
    /// Offset in seconds before the due date (negative = before).
    pub relative_offset: Option<f64>,
    /// Absolute date for the alarm (ISO 8601 string).
    pub absolute_date: Option<DateTime<Local>>,
    /// Proximity trigger (enter/leave geofence).
    pub proximity: AlarmProximity,
    /// Location for geofenced alarms.
    pub location: Option<StructuredLocation>,
}

/// How often a recurrence repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// When a recurrence ends.
#[derive(Debug, Clone)]
pub enum RecurrenceEndCondition {
    /// Repeats forever.
    Never,
    /// Ends after a number of occurrences.
    AfterCount(usize),
    /// Ends on a specific date.
    OnDate(DateTime<Local>),
}

/// A recurrence rule describing how a reminder or event repeats.
#[derive(Debug, Clone)]
pub struct RecurrenceRule {
    /// How often it repeats (daily, weekly, monthly, yearly).
    pub frequency: RecurrenceFrequency,
    /// Repeat every N intervals (e.g., every 2 weeks).
    pub interval: usize,
    /// When the recurrence ends.
    pub end: RecurrenceEndCondition,
    /// Days of the week (1=Sun..7=Sat) for weekly/monthly rules.
    pub days_of_week: Option<Vec<u8>>,
    /// Days of the month (1-31, negatives count from end) for monthly rules.
    pub days_of_month: Option<Vec<i32>>,
}

/// The main reminders manager providing access to EventKit functionality
pub struct RemindersManager {
    store: Retained<EKEventStore>,
}

impl RemindersManager {
    /// Creates a new RemindersManager instance
    pub fn new() -> Self {
        let store = unsafe { EKEventStore::new() };
        Self { store }
    }

    /// Gets the current authorization status for reminders
    pub fn authorization_status() -> AuthorizationStatus {
        let status =
            unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Reminder) };
        status.into()
    }

    /// Requests full access to reminders (blocking)
    ///
    /// Returns Ok(true) if access was granted, Ok(false) if denied
    pub fn request_access(&self) -> Result<bool> {
        let result = Arc::new((Mutex::new(None::<(bool, Option<String>)>), Condvar::new()));
        let result_clone = Arc::clone(&result);

        let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let error_msg = if !error.is_null() {
                let error_ref = unsafe { &*error };
                Some(format!("{:?}", error_ref))
            } else {
                None
            };

            let (lock, cvar) = &*result_clone;
            let mut res = lock.lock().unwrap();
            *res = Some((granted.as_bool(), error_msg));
            cvar.notify_one();
        });

        unsafe {
            // Convert RcBlock to raw pointer for the API
            let block_ptr = &*completion as *const _ as *mut _;
            self.store
                .requestFullAccessToRemindersWithCompletion(block_ptr);
        }

        let (lock, cvar) = &*result;
        let mut res = lock.lock().unwrap();
        while res.is_none() {
            res = cvar.wait(res).unwrap();
        }

        match res.take() {
            Some((granted, None)) => Ok(granted),
            Some((_, Some(error))) => Err(RemindersError::AuthorizationRequestFailed(error)),
            None => Err(RemindersError::AuthorizationRequestFailed(
                "Unknown error".to_string(),
            )),
        }
    }

    /// Ensures we have authorization, requesting if needed
    pub fn ensure_authorized(&self) -> Result<()> {
        match Self::authorization_status() {
            AuthorizationStatus::FullAccess => Ok(()),
            AuthorizationStatus::NotDetermined => {
                if self.request_access()? {
                    Ok(())
                } else {
                    Err(RemindersError::AuthorizationDenied)
                }
            }
            AuthorizationStatus::Denied => Err(RemindersError::AuthorizationDenied),
            AuthorizationStatus::Restricted => Err(RemindersError::AuthorizationRestricted),
            AuthorizationStatus::WriteOnly => Ok(()), // Can still read with write-only in some cases
        }
    }

    /// Lists all reminder calendars (lists)
    pub fn list_calendars(&self) -> Result<Vec<CalendarInfo>> {
        self.ensure_authorized()?;

        let calendars = unsafe { self.store.calendarsForEntityType(EKEntityType::Reminder) };

        let mut result = Vec::new();
        for calendar in calendars.iter() {
            result.push(calendar_to_info(&calendar));
        }

        Ok(result)
    }

    /// Lists all available sources (iCloud, Local, Exchange, etc.)
    pub fn list_sources(&self) -> Result<Vec<SourceInfo>> {
        self.ensure_authorized()?;
        let sources = unsafe { self.store.sources() };
        let mut result = Vec::new();
        for source in sources.iter() {
            result.push(source_to_info(&source));
        }
        Ok(result)
    }

    /// Gets the default calendar for new reminders
    pub fn default_calendar(&self) -> Result<CalendarInfo> {
        self.ensure_authorized()?;

        let calendar = unsafe { self.store.defaultCalendarForNewReminders() };

        match calendar {
            Some(cal) => Ok(calendar_to_info(&cal)),
            None => Err(RemindersError::NoDefaultCalendar),
        }
    }

    /// Fetches all reminders (blocking)
    pub fn fetch_all_reminders(&self) -> Result<Vec<ReminderItem>> {
        self.fetch_reminders(None)
    }

    /// Fetches reminders from specific calendars (blocking)
    pub fn fetch_reminders(&self, calendar_titles: Option<&[&str]>) -> Result<Vec<ReminderItem>> {
        self.ensure_authorized()?;

        let calendars: Option<Retained<NSArray<EKCalendar>>> = match calendar_titles {
            Some(titles) => {
                let all_calendars =
                    unsafe { self.store.calendarsForEntityType(EKEntityType::Reminder) };
                let mut matching: Vec<Retained<EKCalendar>> = Vec::new();

                for cal in all_calendars.iter() {
                    let title = unsafe { cal.title() };
                    let title_str = title.to_string();
                    if titles.iter().any(|t| *t == title_str) {
                        matching.push(cal.retain());
                    }
                }

                if matching.is_empty() {
                    return Err(RemindersError::CalendarNotFound(titles.join(", ")));
                }

                Some(NSArray::from_retained_slice(&matching))
            }
            None => None,
        };

        let predicate = unsafe {
            self.store
                .predicateForRemindersInCalendars(calendars.as_deref())
        };

        let result = Arc::new((Mutex::new(None::<Vec<ReminderItem>>), Condvar::new()));
        let result_clone = Arc::clone(&result);

        let completion = RcBlock::new(move |reminders: *mut NSArray<EKReminder>| {
            let items = if reminders.is_null() {
                Vec::new()
            } else {
                let reminders = unsafe { Retained::retain(reminders).unwrap() };
                reminders.iter().map(|r| reminder_to_item(&r)).collect()
            };
            let (lock, cvar) = &*result_clone;
            let mut guard = lock.lock().unwrap();
            *guard = Some(items);
            cvar.notify_one();
        });

        unsafe {
            self.store
                .fetchRemindersMatchingPredicate_completion(&predicate, &completion);
        }

        let (lock, cvar) = &*result;
        let mut guard = lock.lock().unwrap();
        while guard.is_none() {
            guard = cvar.wait(guard).unwrap();
        }

        guard
            .take()
            .ok_or_else(|| RemindersError::FetchFailed("Unknown error".to_string()))
    }

    /// Fetches incomplete reminders
    pub fn fetch_incomplete_reminders(&self) -> Result<Vec<ReminderItem>> {
        self.ensure_authorized()?;

        let predicate = unsafe {
            self.store
                .predicateForIncompleteRemindersWithDueDateStarting_ending_calendars(
                    None, None, None,
                )
        };

        let result = Arc::new((Mutex::new(None::<Vec<ReminderItem>>), Condvar::new()));
        let result_clone = Arc::clone(&result);

        let completion = RcBlock::new(move |reminders: *mut NSArray<EKReminder>| {
            let items = if reminders.is_null() {
                Vec::new()
            } else {
                let reminders = unsafe { Retained::retain(reminders).unwrap() };
                reminders.iter().map(|r| reminder_to_item(&r)).collect()
            };
            let (lock, cvar) = &*result_clone;
            let mut guard = lock.lock().unwrap();
            *guard = Some(items);
            cvar.notify_one();
        });

        unsafe {
            self.store
                .fetchRemindersMatchingPredicate_completion(&predicate, &completion);
        }

        let (lock, cvar) = &*result;
        let mut guard = lock.lock().unwrap();
        while guard.is_none() {
            guard = cvar.wait(guard).unwrap();
        }

        guard
            .take()
            .ok_or_else(|| RemindersError::FetchFailed("Unknown error".to_string()))
    }

    /// Creates a new reminder
    ///
    /// # Arguments
    /// * `title` - The reminder title
    /// * `notes` - Optional notes/description
    /// * `calendar_title` - Optional calendar/list name (uses default if None)
    /// * `priority` - Optional priority (0 = none, 1-4 = high, 5 = medium, 6-9 = low)
    /// * `due_date` - Optional due date for the reminder
    /// * `start_date` - Optional start date (when to start working on it)
    #[allow(clippy::too_many_arguments)]
    pub fn create_reminder(
        &self,
        title: &str,
        notes: Option<&str>,
        calendar_title: Option<&str>,
        priority: Option<usize>,
        due_date: Option<DateTime<Local>>,
        start_date: Option<DateTime<Local>>,
    ) -> Result<ReminderItem> {
        self.ensure_authorized()?;

        let reminder = unsafe { EKReminder::reminderWithEventStore(&self.store) };

        // Set title
        let ns_title = NSString::from_str(title);
        unsafe { reminder.setTitle(Some(&ns_title)) };

        // Set notes if provided
        if let Some(notes_text) = notes {
            let ns_notes = NSString::from_str(notes_text);
            unsafe { reminder.setNotes(Some(&ns_notes)) };
        }

        // Set priority if provided
        if let Some(p) = priority {
            unsafe { reminder.setPriority(p) };
        }

        // Set due date if provided
        if let Some(due) = due_date {
            let components = datetime_to_date_components(due);
            unsafe { reminder.setDueDateComponents(Some(&components)) };
        }

        // Set start date if provided
        if let Some(start) = start_date {
            let components = datetime_to_date_components(start);
            unsafe { reminder.setStartDateComponents(Some(&components)) };
        }

        // Set calendar
        let calendar = if let Some(cal_title) = calendar_title {
            self.find_calendar_by_title(cal_title)?
        } else {
            unsafe { self.store.defaultCalendarForNewReminders() }
                .ok_or(RemindersError::NoDefaultCalendar)?
        };
        unsafe { reminder.setCalendar(Some(&calendar)) };

        // Save
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| RemindersError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(reminder_to_item(&reminder))
    }

    /// Updates an existing reminder
    ///
    /// All fields are optional - only provided fields will be updated.
    /// Pass `Some(None)` for due_date/start_date to clear them.
    /// Use `calendar_title` to move the reminder to a different list.
    #[allow(clippy::too_many_arguments)]
    pub fn update_reminder(
        &self,
        identifier: &str,
        title: Option<&str>,
        notes: Option<&str>,
        completed: Option<bool>,
        priority: Option<usize>,
        due_date: Option<Option<DateTime<Local>>>,
        start_date: Option<Option<DateTime<Local>>>,
        calendar_title: Option<&str>,
    ) -> Result<ReminderItem> {
        self.ensure_authorized()?;

        let reminder = self.find_reminder_by_id(identifier)?;

        if let Some(t) = title {
            let ns_title = NSString::from_str(t);
            unsafe { reminder.setTitle(Some(&ns_title)) };
        }

        if let Some(n) = notes {
            let ns_notes = NSString::from_str(n);
            unsafe { reminder.setNotes(Some(&ns_notes)) };
        }

        if let Some(c) = completed {
            unsafe { reminder.setCompleted(c) };
        }

        if let Some(p) = priority {
            unsafe { reminder.setPriority(p) };
        }

        // Handle due date: Some(Some(date)) = set, Some(None) = clear, None = no change
        if let Some(due_opt) = due_date {
            match due_opt {
                Some(due) => {
                    let components = datetime_to_date_components(due);
                    unsafe { reminder.setDueDateComponents(Some(&components)) };
                }
                None => {
                    unsafe { reminder.setDueDateComponents(None) };
                }
            }
        }

        // Handle start date: Some(Some(date)) = set, Some(None) = clear, None = no change
        if let Some(start_opt) = start_date {
            match start_opt {
                Some(start) => {
                    let components = datetime_to_date_components(start);
                    unsafe { reminder.setStartDateComponents(Some(&components)) };
                }
                None => {
                    unsafe { reminder.setStartDateComponents(None) };
                }
            }
        }

        // Move to a different calendar/list if specified
        if let Some(cal_title) = calendar_title {
            let calendar = self.find_calendar_by_title(cal_title)?;
            unsafe { reminder.setCalendar(Some(&calendar)) };
        }

        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| RemindersError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(reminder_to_item(&reminder))
    }

    /// Marks a reminder as complete
    pub fn complete_reminder(&self, identifier: &str) -> Result<ReminderItem> {
        self.update_reminder(identifier, None, None, Some(true), None, None, None, None)
    }

    /// Marks a reminder as incomplete
    pub fn uncomplete_reminder(&self, identifier: &str) -> Result<ReminderItem> {
        self.update_reminder(identifier, None, None, Some(false), None, None, None, None)
    }

    /// Deletes a reminder
    pub fn delete_reminder(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;

        let reminder = self.find_reminder_by_id(identifier)?;

        unsafe {
            self.store
                .removeReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::DeleteFailed(format!("{:?}", e)))?;
        }

        Ok(())
    }

    /// Gets a reminder by its identifier
    pub fn get_reminder(&self, identifier: &str) -> Result<ReminderItem> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        Ok(reminder_to_item(&reminder))
    }

    // ========================================================================
    // Alarm Management
    // ========================================================================

    /// Lists all alarms on a reminder.
    pub fn get_alarms(&self, identifier: &str) -> Result<Vec<AlarmInfo>> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        Ok(get_item_alarms(&reminder))
    }

    /// Adds an alarm to a reminder.
    pub fn add_alarm(&self, identifier: &str, alarm: &AlarmInfo) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        add_item_alarm(&reminder, alarm);
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Removes all alarms from a reminder.
    pub fn remove_all_alarms(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        clear_item_alarms(&reminder);
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Removes a specific alarm from a reminder by index.
    pub fn remove_alarm(&self, identifier: &str, index: usize) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        remove_item_alarm(&reminder, index)?;
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // ========================================================================
    // URL Management
    // ========================================================================

    /// Set or clear the URL on a reminder.
    pub fn set_url(&self, identifier: &str, url: Option<&str>) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        set_item_url(&reminder, url);
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // ========================================================================
    // Recurrence Rule Management
    // ========================================================================

    /// Gets recurrence rules on a reminder.
    pub fn get_recurrence_rules(&self, identifier: &str) -> Result<Vec<RecurrenceRule>> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        Ok(get_item_recurrence_rules(&reminder))
    }

    /// Sets a recurrence rule on a reminder (replaces any existing rules).
    pub fn set_recurrence_rule(&self, identifier: &str, rule: &RecurrenceRule) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        set_item_recurrence_rule(&reminder, rule);
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Removes all recurrence rules from a reminder.
    pub fn remove_recurrence_rules(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;
        let reminder = self.find_reminder_by_id(identifier)?;
        clear_item_recurrence_rules(&reminder);
        unsafe {
            self.store
                .saveReminder_commit_error(&reminder, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // ========================================================================
    // Calendar (Reminder List) Management
    // ========================================================================

    /// Creates a new reminder list (calendar)
    ///
    /// The list will be created in the default source (usually iCloud or Local).
    pub fn create_calendar(&self, title: &str) -> Result<CalendarInfo> {
        self.ensure_authorized()?;

        // Create a new calendar for reminders
        let calendar = unsafe {
            EKCalendar::calendarForEntityType_eventStore(EKEntityType::Reminder, &self.store)
        };

        // Set the title
        let ns_title = NSString::from_str(title);
        unsafe { calendar.setTitle(&ns_title) };

        // Find a suitable source (prefer iCloud, fall back to local)
        let source = self.find_best_source_for_reminders()?;
        unsafe { calendar.setSource(Some(&source)) };

        // Save the calendar
        unsafe {
            self.store
                .saveCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(calendar_to_info(&calendar))
    }

    /// Renames an existing reminder list (calendar)
    /// Rename a reminder list (backward compat wrapper).
    pub fn rename_calendar(&self, identifier: &str, new_title: &str) -> Result<CalendarInfo> {
        self.update_calendar(identifier, Some(new_title), None)
    }

    /// Update a reminder list — name, color, or both.
    pub fn update_calendar(
        &self,
        identifier: &str,
        new_title: Option<&str>,
        color_rgba: Option<(f64, f64, f64, f64)>,
    ) -> Result<CalendarInfo> {
        self.ensure_authorized()?;
        let calendar = self.find_calendar_by_id(identifier)?;

        if !unsafe { calendar.allowsContentModifications() } {
            return Err(EventKitError::SaveFailed(
                "Calendar does not allow modifications".to_string(),
            ));
        }

        if let Some(title) = new_title {
            let ns_title = NSString::from_str(title);
            unsafe { calendar.setTitle(&ns_title) };
        }

        if let Some((r, g, b, a)) = color_rgba {
            let cg = objc2_core_graphics::CGColor::new_srgb(r, g, b, a);
            unsafe { calendar.setCGColor(Some(&cg)) };
        }

        unsafe {
            self.store
                .saveCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(calendar_to_info(&calendar))
    }

    /// Deletes a reminder list (calendar)
    ///
    /// Warning: This will delete all reminders in the list!
    pub fn delete_calendar(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;

        let calendar = self.find_calendar_by_id(identifier)?;

        // Check if modifications are allowed
        if !unsafe { calendar.allowsContentModifications() } {
            return Err(EventKitError::DeleteFailed(
                "Calendar does not allow modifications".to_string(),
            ));
        }

        unsafe {
            self.store
                .removeCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::DeleteFailed(format!("{:?}", e)))?;
        }

        Ok(())
    }

    /// Gets a calendar by its identifier
    pub fn get_calendar(&self, identifier: &str) -> Result<CalendarInfo> {
        self.ensure_authorized()?;
        let calendar = self.find_calendar_by_id(identifier)?;
        Ok(calendar_to_info(&calendar))
    }

    // Helper to find the best source for creating new reminder calendars
    fn find_best_source_for_reminders(&self) -> Result<Retained<objc2_event_kit::EKSource>> {
        // Try to get the source from the default calendar first
        if let Some(default_cal) = unsafe { self.store.defaultCalendarForNewReminders() }
            && let Some(source) = unsafe { default_cal.source() }
        {
            return Ok(source);
        }

        // Fall back to finding any source that supports reminders
        let sources = unsafe { self.store.sources() };
        for source in sources.iter() {
            // Check if this source supports reminder calendars
            let calendars = unsafe { source.calendarsForEntityType(EKEntityType::Reminder) };
            if !calendars.is_empty() {
                return Ok(source.retain());
            }
        }

        Err(EventKitError::SaveFailed(
            "No suitable source found for creating reminder calendar".to_string(),
        ))
    }

    // Helper to find a calendar by identifier
    fn find_calendar_by_id(&self, identifier: &str) -> Result<Retained<EKCalendar>> {
        let ns_id = NSString::from_str(identifier);
        let calendar = unsafe { self.store.calendarWithIdentifier(&ns_id) };

        match calendar {
            Some(cal) => Ok(cal),
            None => Err(EventKitError::CalendarNotFound(identifier.to_string())),
        }
    }

    // Helper to find a calendar by title
    fn find_calendar_by_title(&self, title: &str) -> Result<Retained<EKCalendar>> {
        let calendars = unsafe { self.store.calendarsForEntityType(EKEntityType::Reminder) };

        for cal in calendars.iter() {
            let cal_title = unsafe { cal.title() };
            if cal_title.to_string() == title {
                return Ok(cal.retain());
            }
        }

        Err(RemindersError::CalendarNotFound(title.to_string()))
    }

    // Helper to find a reminder by identifier
    fn find_reminder_by_id(&self, identifier: &str) -> Result<Retained<EKReminder>> {
        let ns_id = NSString::from_str(identifier);
        let item = unsafe { self.store.calendarItemWithIdentifier(&ns_id) };

        match item {
            Some(item) => {
                // Try to downcast to EKReminder
                if let Some(reminder) = item.downcast_ref::<EKReminder>() {
                    Ok(reminder.retain())
                } else {
                    Err(EventKitError::ItemNotFound(identifier.to_string()))
                }
            }
            None => Err(EventKitError::ItemNotFound(identifier.to_string())),
        }
    }
}

impl Default for RemindersManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Authorization status for reminders access
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationStatus {
    /// User has not yet made a choice
    NotDetermined,
    /// Access restricted by system policy
    Restricted,
    /// User explicitly denied access
    Denied,
    /// Full access granted
    FullAccess,
    /// Write-only access granted
    WriteOnly,
}

impl From<EKAuthorizationStatus> for AuthorizationStatus {
    fn from(status: EKAuthorizationStatus) -> Self {
        if status == EKAuthorizationStatus::NotDetermined {
            AuthorizationStatus::NotDetermined
        } else if status == EKAuthorizationStatus::Restricted {
            AuthorizationStatus::Restricted
        } else if status == EKAuthorizationStatus::Denied {
            AuthorizationStatus::Denied
        } else if status == EKAuthorizationStatus::FullAccess {
            AuthorizationStatus::FullAccess
        } else if status == EKAuthorizationStatus::WriteOnly {
            AuthorizationStatus::WriteOnly
        } else {
            AuthorizationStatus::NotDetermined
        }
    }
}

impl std::fmt::Display for AuthorizationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthorizationStatus::NotDetermined => write!(f, "Not Determined"),
            AuthorizationStatus::Restricted => write!(f, "Restricted"),
            AuthorizationStatus::Denied => write!(f, "Denied"),
            AuthorizationStatus::FullAccess => write!(f, "Full Access"),
            AuthorizationStatus::WriteOnly => write!(f, "Write Only"),
        }
    }
}

// Helper function to convert EKReminder to ReminderItem
fn reminder_to_item(reminder: &EKReminder) -> ReminderItem {
    let identifier = unsafe { reminder.calendarItemIdentifier() }.to_string();
    let title = unsafe { reminder.title() }.to_string();
    let notes = unsafe { reminder.notes() }.map(|n| n.to_string());
    let completed = unsafe { reminder.isCompleted() };
    let priority = unsafe { reminder.priority() };
    let cal = unsafe { reminder.calendar() };
    let calendar_title = cal.as_ref().map(|c| unsafe { c.title() }.to_string());
    let calendar_id = cal
        .as_ref()
        .map(|c| unsafe { c.calendarIdentifier() }.to_string());

    // Extract due date from dueDateComponents
    let due_date = unsafe { reminder.dueDateComponents() }
        .and_then(|components| date_components_to_datetime(&components));

    // Extract start date from startDateComponents
    let start_date = unsafe { reminder.startDateComponents() }
        .and_then(|components| date_components_to_datetime(&components));

    // Extract completion date
    let completion_date =
        unsafe { reminder.completionDate() }.map(|date| nsdate_to_datetime(&date));

    // Extract additional fields from EKCalendarItem parent class
    let external_identifier =
        unsafe { reminder.calendarItemExternalIdentifier() }.map(|id| id.to_string());
    let location = unsafe { reminder.location() }.map(|loc| loc.to_string());
    let url = unsafe { reminder.URL() }
        .as_ref()
        .and_then(|url_ref| url_ref.absoluteString())
        .map(|abs_str| abs_str.to_string());
    let creation_date = unsafe { reminder.creationDate() }.map(|date| nsdate_to_datetime(&date));
    let last_modified_date =
        unsafe { reminder.lastModifiedDate() }.map(|date| nsdate_to_datetime(&date));
    let timezone = unsafe { reminder.timeZone() }.map(|tz| tz.name().to_string());
    let has_alarms = unsafe { reminder.hasAlarms() };
    let has_recurrence_rules = unsafe { reminder.hasRecurrenceRules() };
    let has_attendees = unsafe { reminder.hasAttendees() };
    let has_notes = unsafe { reminder.hasNotes() };

    ReminderItem {
        identifier,
        title,
        notes,
        completed,
        priority,
        calendar_title,
        calendar_id,
        due_date,
        start_date,
        completion_date,
        external_identifier,
        location,
        url,
        creation_date,
        last_modified_date,
        timezone,
        has_alarms,
        has_recurrence_rules,
        has_attendees,
        has_notes,
        attendees: get_item_attendees(reminder),
    }
}

// Helper function to convert EKCalendar to CalendarInfo
fn source_to_info(source: &EKSource) -> SourceInfo {
    let identifier = unsafe { source.sourceIdentifier() }.to_string();
    let title = unsafe { source.title() }.to_string();
    // EKSourceType: 0=Local, 1=Exchange, 2=CalDAV, 3=MobileMe, 4=Subscribed, 5=Birthdays
    let source_type = unsafe { source.sourceType() };
    let source_type = match source_type.0 {
        0 => "local",
        1 => "exchange",
        2 => "caldav",
        3 => "mobileme",
        4 => "subscribed",
        5 => "birthdays",
        _ => "unknown",
    }
    .to_string();

    SourceInfo {
        identifier,
        title,
        source_type,
    }
}

fn calendar_to_info(calendar: &EKCalendar) -> CalendarInfo {
    let identifier = unsafe { calendar.calendarIdentifier() }.to_string();
    let title = unsafe { calendar.title() }.to_string();
    let source = unsafe { calendar.source() }.map(|s| unsafe { s.title() }.to_string());
    let source_id =
        unsafe { calendar.source() }.map(|s| unsafe { s.sourceIdentifier() }.to_string());
    let allows_modifications = unsafe { calendar.allowsContentModifications() };
    let is_immutable = unsafe { calendar.isImmutable() };
    let is_subscribed = unsafe { calendar.isSubscribed() };

    // Calendar type: Local=0, CalDAV=1, Exchange=2, Subscription=3, Birthday=4
    let cal_type = unsafe { calendar.r#type() };
    let calendar_type = match cal_type.0 {
        0 => CalendarType::Local,
        1 => CalendarType::CalDAV,
        2 => CalendarType::Exchange,
        3 => CalendarType::Subscription,
        4 => CalendarType::Birthday,
        _ => CalendarType::Unknown,
    };

    // Read RGBA from CGColor
    let color: Option<(f64, f64, f64, f64)> = unsafe {
        calendar.CGColor().and_then(|cg| {
            use objc2_core_graphics::CGColor as CG;
            let n = CG::number_of_components(Some(&cg));
            if n >= 3 {
                let ptr = CG::components(Some(&cg));
                let r = *ptr;
                let g = *ptr.add(1);
                let b = *ptr.add(2);
                let a = if n >= 4 { *ptr.add(3) } else { 1.0 };
                Some((r, g, b, a))
            } else {
                None
            }
        })
    };

    // Allowed entity types
    let entity_mask = unsafe { calendar.allowedEntityTypes() };
    let mut allowed_entity_types = Vec::new();
    if entity_mask.0 & 1 != 0 {
        allowed_entity_types.push("event".to_string());
    }
    if entity_mask.0 & 2 != 0 {
        allowed_entity_types.push("reminder".to_string());
    }

    CalendarInfo {
        identifier,
        title,
        source,
        source_id,
        calendar_type,
        allows_modifications,
        is_immutable,
        is_subscribed,
        color,
        allowed_entity_types,
    }
}

// ============================================================================
// Calendar Events Support
// ============================================================================

/// Event availability for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventAvailability {
    NotSupported,
    Busy,
    Free,
    Tentative,
    Unavailable,
}

/// Event status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStatus {
    None,
    Confirmed,
    Tentative,
    Canceled,
}

/// Participant role in an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantRole {
    Unknown,
    Required,
    Optional,
    Chair,
    NonParticipant,
}

/// Participant RSVP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantStatus {
    Unknown,
    Pending,
    Accepted,
    Declined,
    Tentative,
    Delegated,
    Completed,
    InProcess,
}

/// A participant (attendee) on an event or reminder.
#[derive(Debug, Clone)]
pub struct ParticipantInfo {
    pub name: Option<String>,
    pub url: Option<String>,
    pub role: ParticipantRole,
    pub status: ParticipantStatus,
    pub is_current_user: bool,
}

/// Represents a calendar event with its properties.
#[derive(Debug, Clone)]
pub struct EventItem {
    /// Unique identifier for the event
    pub identifier: String,
    /// Title of the event
    pub title: String,
    /// Optional notes/description
    pub notes: Option<String>,
    /// Optional location (string)
    pub location: Option<String>,
    /// Start date/time
    pub start_date: DateTime<Local>,
    /// End date/time
    pub end_date: DateTime<Local>,
    /// Whether this is an all-day event
    pub all_day: bool,
    /// Calendar the event belongs to
    pub calendar_title: Option<String>,
    /// Calendar identifier
    pub calendar_id: Option<String>,
    /// URL associated with the event
    pub url: Option<String>,
    /// Availability for scheduling
    pub availability: EventAvailability,
    /// Event status (read-only)
    pub status: EventStatus,
    /// Whether this occurrence was modified from its recurring series
    pub is_detached: bool,
    /// Original date in a recurring series
    pub occurrence_date: Option<DateTime<Local>>,
    /// Geo-coordinate location
    pub structured_location: Option<StructuredLocation>,
    /// Attendees
    pub attendees: Vec<ParticipantInfo>,
    /// Event organizer
    pub organizer: Option<ParticipantInfo>,
}

/// The events manager providing access to Calendar events via EventKit
pub struct EventsManager {
    store: Retained<EKEventStore>,
}

impl EventsManager {
    /// Creates a new EventsManager instance
    pub fn new() -> Self {
        let store = unsafe { EKEventStore::new() };
        Self { store }
    }

    /// Gets the current authorization status for calendar events
    pub fn authorization_status() -> AuthorizationStatus {
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        status.into()
    }

    /// Requests full access to calendar events (blocking)
    ///
    /// Returns Ok(true) if access was granted, Ok(false) if denied
    pub fn request_access(&self) -> Result<bool> {
        let result = Arc::new((Mutex::new(None::<(bool, Option<String>)>), Condvar::new()));
        let result_clone = Arc::clone(&result);

        let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let error_msg = if !error.is_null() {
                let error_ref = unsafe { &*error };
                Some(format!("{:?}", error_ref))
            } else {
                None
            };

            let (lock, cvar) = &*result_clone;
            let mut res = lock.lock().unwrap();
            *res = Some((granted.as_bool(), error_msg));
            cvar.notify_one();
        });

        unsafe {
            let block_ptr = &*completion as *const _ as *mut _;
            self.store
                .requestFullAccessToEventsWithCompletion(block_ptr);
        }

        let (lock, cvar) = &*result;
        let mut res = lock.lock().unwrap();
        while res.is_none() {
            res = cvar.wait(res).unwrap();
        }

        match res.take() {
            Some((granted, None)) => Ok(granted),
            Some((_, Some(error))) => Err(EventKitError::AuthorizationRequestFailed(error)),
            None => Err(EventKitError::AuthorizationRequestFailed(
                "Unknown error".to_string(),
            )),
        }
    }

    /// Ensures we have authorization, requesting if needed
    pub fn ensure_authorized(&self) -> Result<()> {
        match Self::authorization_status() {
            AuthorizationStatus::FullAccess => Ok(()),
            AuthorizationStatus::NotDetermined => {
                if self.request_access()? {
                    Ok(())
                } else {
                    Err(EventKitError::AuthorizationDenied)
                }
            }
            AuthorizationStatus::Denied => Err(EventKitError::AuthorizationDenied),
            AuthorizationStatus::Restricted => Err(EventKitError::AuthorizationRestricted),
            AuthorizationStatus::WriteOnly => Ok(()),
        }
    }

    /// Lists all event calendars
    pub fn list_calendars(&self) -> Result<Vec<CalendarInfo>> {
        self.ensure_authorized()?;

        let calendars = unsafe { self.store.calendarsForEntityType(EKEntityType::Event) };

        let mut result = Vec::new();
        for calendar in calendars.iter() {
            result.push(calendar_to_info(&calendar));
        }

        Ok(result)
    }

    /// Gets the default calendar for new events
    pub fn default_calendar(&self) -> Result<CalendarInfo> {
        self.ensure_authorized()?;

        let calendar = unsafe { self.store.defaultCalendarForNewEvents() };

        match calendar {
            Some(cal) => Ok(calendar_to_info(&cal)),
            None => Err(EventKitError::NoDefaultCalendar),
        }
    }

    /// Fetches events for today
    pub fn fetch_today_events(&self) -> Result<Vec<EventItem>> {
        let now = Local::now();
        let start = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
        let end = now.date_naive().and_hms_opt(23, 59, 59).unwrap();

        self.fetch_events(
            Local.from_local_datetime(&start).unwrap(),
            Local.from_local_datetime(&end).unwrap(),
            None,
        )
    }

    /// Fetches events for the next N days
    pub fn fetch_upcoming_events(&self, days: i64) -> Result<Vec<EventItem>> {
        let now = Local::now();
        let end = now + Duration::days(days);
        self.fetch_events(now, end, None)
    }

    /// Fetches events in a date range
    pub fn fetch_events(
        &self,
        start: DateTime<Local>,
        end: DateTime<Local>,
        calendar_titles: Option<&[&str]>,
    ) -> Result<Vec<EventItem>> {
        self.ensure_authorized()?;

        if start >= end {
            return Err(EventKitError::InvalidDateRange);
        }

        let calendars: Option<Retained<NSArray<EKCalendar>>> = match calendar_titles {
            Some(titles) => {
                let all_calendars =
                    unsafe { self.store.calendarsForEntityType(EKEntityType::Event) };
                let mut matching: Vec<Retained<EKCalendar>> = Vec::new();

                for cal in all_calendars.iter() {
                    let title = unsafe { cal.title() };
                    let title_str = title.to_string();
                    if titles.iter().any(|t| *t == title_str) {
                        matching.push(cal.retain());
                    }
                }

                if matching.is_empty() {
                    return Err(EventKitError::CalendarNotFound(titles.join(", ")));
                }

                Some(NSArray::from_retained_slice(&matching))
            }
            None => None,
        };

        let start_date = datetime_to_nsdate(start);
        let end_date = datetime_to_nsdate(end);

        let predicate = unsafe {
            self.store
                .predicateForEventsWithStartDate_endDate_calendars(
                    &start_date,
                    &end_date,
                    calendars.as_deref(),
                )
        };

        let events = unsafe { self.store.eventsMatchingPredicate(&predicate) };

        let mut items = Vec::new();
        for event in events.iter() {
            items.push(event_to_item(&event));
        }

        // Sort by start date
        items.sort_by(|a, b| a.start_date.cmp(&b.start_date));

        Ok(items)
    }

    /// Creates a new event
    #[allow(clippy::too_many_arguments)]
    pub fn create_event(
        &self,
        title: &str,
        start: DateTime<Local>,
        end: DateTime<Local>,
        notes: Option<&str>,
        location: Option<&str>,
        calendar_title: Option<&str>,
        all_day: bool,
    ) -> Result<EventItem> {
        self.ensure_authorized()?;

        let event = unsafe { EKEvent::eventWithEventStore(&self.store) };

        // Set title
        let ns_title = NSString::from_str(title);
        unsafe { event.setTitle(Some(&ns_title)) };

        // Set dates
        let start_date = datetime_to_nsdate(start);
        let end_date = datetime_to_nsdate(end);
        unsafe {
            event.setStartDate(Some(&start_date));
            event.setEndDate(Some(&end_date));
            event.setAllDay(all_day);
        }

        // Set notes if provided
        if let Some(notes_text) = notes {
            let ns_notes = NSString::from_str(notes_text);
            unsafe { event.setNotes(Some(&ns_notes)) };
        }

        // Set location if provided
        if let Some(loc) = location {
            let ns_location = NSString::from_str(loc);
            unsafe { event.setLocation(Some(&ns_location)) };
        }

        // Set calendar
        let calendar = if let Some(cal_title) = calendar_title {
            self.find_calendar_by_title(cal_title)?
        } else {
            unsafe { self.store.defaultCalendarForNewEvents() }
                .ok_or(EventKitError::NoDefaultCalendar)?
        };
        unsafe { event.setCalendar(Some(&calendar)) };

        // Save
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(event_to_item(&event))
    }

    /// Updates an existing event
    pub fn update_event(
        &self,
        identifier: &str,
        title: Option<&str>,
        notes: Option<&str>,
        location: Option<&str>,
        start: Option<DateTime<Local>>,
        end: Option<DateTime<Local>>,
    ) -> Result<EventItem> {
        self.ensure_authorized()?;

        let event = self.find_event_by_id(identifier)?;

        if let Some(t) = title {
            let ns_title = NSString::from_str(t);
            unsafe { event.setTitle(Some(&ns_title)) };
        }

        if let Some(n) = notes {
            let ns_notes = NSString::from_str(n);
            unsafe { event.setNotes(Some(&ns_notes)) };
        }

        if let Some(l) = location {
            let ns_location = NSString::from_str(l);
            unsafe { event.setLocation(Some(&ns_location)) };
        }

        if let Some(s) = start {
            let start_date = datetime_to_nsdate(s);
            unsafe { event.setStartDate(Some(&start_date)) };
        }

        if let Some(e) = end {
            let end_date = datetime_to_nsdate(e);
            unsafe { event.setEndDate(Some(&end_date)) };
        }

        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }

        Ok(event_to_item(&event))
    }

    /// Deletes an event
    pub fn delete_event(&self, identifier: &str, affect_future: bool) -> Result<()> {
        self.ensure_authorized()?;

        let event = self.find_event_by_id(identifier)?;
        let span = if affect_future {
            EKSpan::FutureEvents
        } else {
            EKSpan::ThisEvent
        };

        unsafe {
            self.store
                .removeEvent_span_commit_error(&event, span, true)
                .map_err(|e| EventKitError::DeleteFailed(format!("{:?}", e)))?;
        }

        Ok(())
    }

    /// Gets an event by its identifier
    pub fn get_event(&self, identifier: &str) -> Result<EventItem> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        Ok(event_to_item(&event))
    }

    // ========================================================================
    // Event Calendar Management
    // ========================================================================

    /// Creates a new event calendar.
    pub fn create_event_calendar(&self, title: &str) -> Result<CalendarInfo> {
        self.ensure_authorized()?;
        let calendar = unsafe {
            EKCalendar::calendarForEntityType_eventStore(EKEntityType::Event, &self.store)
        };
        let ns_title = NSString::from_str(title);
        unsafe { calendar.setTitle(&ns_title) };

        // Use the default source
        if let Some(default_cal) = unsafe { self.store.defaultCalendarForNewEvents() }
            && let Some(source) = unsafe { default_cal.source() }
        {
            unsafe { calendar.setSource(Some(&source)) };
        }

        unsafe {
            self.store
                .saveCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(calendar_to_info(&calendar))
    }

    /// Renames an event calendar.
    /// Rename an event calendar (backward compat wrapper).
    pub fn rename_event_calendar(&self, identifier: &str, new_title: &str) -> Result<CalendarInfo> {
        self.update_event_calendar(identifier, Some(new_title), None)
    }

    /// Update an event calendar — name, color, or both.
    pub fn update_event_calendar(
        &self,
        identifier: &str,
        new_title: Option<&str>,
        color_rgba: Option<(f64, f64, f64, f64)>,
    ) -> Result<CalendarInfo> {
        self.ensure_authorized()?;
        let calendar = unsafe {
            self.store
                .calendarWithIdentifier(&NSString::from_str(identifier))
        }
        .ok_or_else(|| EventKitError::CalendarNotFound(identifier.to_string()))?;

        if let Some(title) = new_title {
            let ns_title = NSString::from_str(title);
            unsafe { calendar.setTitle(&ns_title) };
        }

        if let Some((r, g, b, a)) = color_rgba {
            let cg = objc2_core_graphics::CGColor::new_srgb(r, g, b, a);
            unsafe { calendar.setCGColor(Some(&cg)) };
        }

        unsafe {
            self.store
                .saveCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(calendar_to_info(&calendar))
    }

    /// Deletes an event calendar.
    pub fn delete_event_calendar(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;
        let calendar = unsafe {
            self.store
                .calendarWithIdentifier(&NSString::from_str(identifier))
        }
        .ok_or_else(|| EventKitError::CalendarNotFound(identifier.to_string()))?;

        unsafe {
            self.store
                .removeCalendar_commit_error(&calendar, true)
                .map_err(|e| EventKitError::DeleteFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // ========================================================================
    // Event Alarm Management (shared via EKCalendarItem)
    // ========================================================================

    /// Lists all alarms on an event.
    pub fn get_event_alarms(&self, identifier: &str) -> Result<Vec<AlarmInfo>> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        Ok(get_item_alarms(&event))
    }

    /// Adds an alarm to an event.
    pub fn add_event_alarm(&self, identifier: &str, alarm: &AlarmInfo) -> Result<()> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        add_item_alarm(&event, alarm);
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // ========================================================================
    // Event Recurrence Management (shared via EKCalendarItem)
    // ========================================================================

    /// Gets recurrence rules on an event.
    pub fn get_event_recurrence_rules(&self, identifier: &str) -> Result<Vec<RecurrenceRule>> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        Ok(get_item_recurrence_rules(&event))
    }

    /// Sets a recurrence rule on an event (replaces any existing rules).
    pub fn set_event_recurrence_rule(&self, identifier: &str, rule: &RecurrenceRule) -> Result<()> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        set_item_recurrence_rule(&event, rule);
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Removes all recurrence rules from an event.
    pub fn remove_event_recurrence_rules(&self, identifier: &str) -> Result<()> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        clear_item_recurrence_rules(&event);
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Removes a specific alarm from an event by index.
    pub fn remove_event_alarm(&self, identifier: &str, index: usize) -> Result<()> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        remove_item_alarm(&event, index)?;
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    /// Set or clear the URL on an event.
    pub fn set_event_url(&self, identifier: &str, url: Option<&str>) -> Result<()> {
        self.ensure_authorized()?;
        let event = self.find_event_by_id(identifier)?;
        set_item_url(&event, url);
        unsafe {
            self.store
                .saveEvent_span_commit_error(&event, EKSpan::ThisEvent, true)
                .map_err(|e| EventKitError::SaveFailed(format!("{:?}", e)))?;
        }
        Ok(())
    }

    // Helper to find a calendar by title
    fn find_calendar_by_title(&self, title: &str) -> Result<Retained<EKCalendar>> {
        let calendars = unsafe { self.store.calendarsForEntityType(EKEntityType::Event) };

        for cal in calendars.iter() {
            let cal_title = unsafe { cal.title() };
            if cal_title.to_string() == title {
                return Ok(cal.retain());
            }
        }

        Err(EventKitError::CalendarNotFound(title.to_string()))
    }

    // Helper to find an event by identifier
    fn find_event_by_id(&self, identifier: &str) -> Result<Retained<EKEvent>> {
        let ns_id = NSString::from_str(identifier);
        let event = unsafe { self.store.eventWithIdentifier(&ns_id) };

        match event {
            Some(e) => Ok(e),
            None => Err(EventKitError::ItemNotFound(identifier.to_string())),
        }
    }
}

impl Default for EventsManager {
    fn default() -> Self {
        Self::new()
    }
}

// Helper function to convert EKEvent to EventItem
fn event_to_item(event: &EKEvent) -> EventItem {
    let identifier = unsafe { event.eventIdentifier() }
        .map(|s| s.to_string())
        .unwrap_or_default();
    let title = unsafe { event.title() }.to_string();
    let notes = unsafe { event.notes() }.map(|n| n.to_string());
    let location = unsafe { event.location() }.map(|l| l.to_string());
    let all_day = unsafe { event.isAllDay() };
    let cal = unsafe { event.calendar() };
    let calendar_title = cal.as_ref().map(|c| unsafe { c.title() }.to_string());
    let calendar_id = cal
        .as_ref()
        .map(|c| unsafe { c.calendarIdentifier() }.to_string());

    let start_ns: Retained<NSDate> = unsafe { event.startDate() };
    let end_ns: Retained<NSDate> = unsafe { event.endDate() };
    let start_date = nsdate_to_datetime(&start_ns);
    let end_date = nsdate_to_datetime(&end_ns);

    let url = get_item_url(event);

    // Availability: -1=NotSupported, 0=Busy, 1=Free, 2=Tentative, 3=Unavailable
    let avail = unsafe { event.availability() };
    let availability = match avail.0 {
        0 => EventAvailability::Busy,
        1 => EventAvailability::Free,
        2 => EventAvailability::Tentative,
        3 => EventAvailability::Unavailable,
        _ => EventAvailability::NotSupported,
    };

    // Status: 0=None, 1=Confirmed, 2=Tentative, 3=Canceled
    let stat = unsafe { event.status() };
    let status = match stat.0 {
        1 => EventStatus::Confirmed,
        2 => EventStatus::Tentative,
        3 => EventStatus::Canceled,
        _ => EventStatus::None,
    };

    let is_detached = unsafe { event.isDetached() };
    let occurrence_date = unsafe { event.occurrenceDate() }.map(|d| nsdate_to_datetime(&d));

    // Structured location
    let structured_location = unsafe { event.structuredLocation() }.map(|loc| {
        let title = unsafe { loc.title() }
            .map(|t| t.to_string())
            .unwrap_or_default();
        let radius = unsafe { loc.radius() };
        let (latitude, longitude) = unsafe { loc.geoLocation() }
            .map(|geo| {
                let coord = unsafe { geo.coordinate() };
                (coord.latitude, coord.longitude)
            })
            .unwrap_or((0.0, 0.0));
        StructuredLocation {
            title,
            latitude,
            longitude,
            radius,
        }
    });

    // Attendees (shared via EKCalendarItem)
    let attendees = get_item_attendees(event);

    // Organizer (event-only)
    let organizer = unsafe { event.organizer() }.map(|p| participant_to_info(&p));

    EventItem {
        identifier,
        title,
        notes,
        location,
        start_date,
        end_date,
        all_day,
        calendar_title,
        calendar_id,
        url,
        availability,
        status,
        is_detached,
        occurrence_date,
        structured_location,
        attendees,
        organizer,
    }
}

// Read attendees from an EKCalendarItem (shared by events and reminders)
fn get_item_attendees(item: &EKCalendarItem) -> Vec<ParticipantInfo> {
    let attendees = unsafe { item.attendees() };
    let Some(attendees) = attendees else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for i in 0..attendees.len() {
        let p = attendees.objectAtIndex(i);
        result.push(participant_to_info(&p));
    }
    result
}

// Convert an EKParticipant to ParticipantInfo
fn participant_to_info(p: &objc2_event_kit::EKParticipant) -> ParticipantInfo {
    let name = unsafe { p.name() }.map(|n| n.to_string());
    let url = unsafe { p.URL() }.absoluteString().map(|s| s.to_string());

    // Role: 0=Unknown, 1=Required, 2=Optional, 3=Chair, 4=NonParticipant
    let role = unsafe { p.participantRole() };
    let role = match role.0 {
        1 => ParticipantRole::Required,
        2 => ParticipantRole::Optional,
        3 => ParticipantRole::Chair,
        4 => ParticipantRole::NonParticipant,
        _ => ParticipantRole::Unknown,
    };

    // Status: 0=Unknown, 1=Pending, 2=Accepted, 3=Declined, 4=Tentative,
    //         5=Delegated, 6=Completed, 7=InProcess
    let status = unsafe { p.participantStatus() };
    let status = match status.0 {
        1 => ParticipantStatus::Pending,
        2 => ParticipantStatus::Accepted,
        3 => ParticipantStatus::Declined,
        4 => ParticipantStatus::Tentative,
        5 => ParticipantStatus::Delegated,
        6 => ParticipantStatus::Completed,
        7 => ParticipantStatus::InProcess,
        _ => ParticipantStatus::Unknown,
    };

    let is_current_user = unsafe { p.isCurrentUser() };

    ParticipantInfo {
        name,
        url,
        role,
        status,
        is_current_user,
    }
}

// Helper to convert chrono DateTime to NSDate
fn datetime_to_nsdate(dt: DateTime<Local>) -> Retained<NSDate> {
    let timestamp = dt.timestamp() as f64;
    NSDate::dateWithTimeIntervalSince1970(timestamp)
}

// Helper to convert NSDate to chrono DateTime
fn nsdate_to_datetime(date: &NSDate) -> DateTime<Local> {
    let timestamp = date.timeIntervalSince1970();
    Local.timestamp_opt(timestamp as i64, 0).unwrap()
}

// Helper to convert NSDateComponents to chrono DateTime
fn date_components_to_datetime(components: &NSDateComponents) -> Option<DateTime<Local>> {
    // Get a calendar to convert components to a date
    let calendar = NSCalendar::currentCalendar();

    // Convert components to NSDate using the calendar
    let date = calendar.dateFromComponents(components)?;

    Some(nsdate_to_datetime(&date))
}

// Helper to convert chrono DateTime to NSDateComponents
fn datetime_to_date_components(dt: DateTime<Local>) -> Retained<NSDateComponents> {
    let components = NSDateComponents::new();

    components.setYear(dt.year() as isize);
    components.setMonth(dt.month() as isize);
    components.setDay(dt.day() as isize);
    components.setHour(dt.hour() as isize);
    components.setMinute(dt.minute() as isize);
    components.setSecond(dt.second() as isize);

    components
}

// ============================================================================
// Shared EKCalendarItem operations
// ============================================================================
// EKCalendarItem is the base class for both EKReminder and EKEvent.
// These functions operate on the shared interface — both types auto-deref to it.

/// Read all alarms from a calendar item.
fn get_item_alarms(item: &EKCalendarItem) -> Vec<AlarmInfo> {
    let alarms = unsafe { item.alarms() };
    let Some(alarms) = alarms else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for i in 0..alarms.len() {
        let alarm = alarms.objectAtIndex(i);
        result.push(alarm_to_info(&alarm));
    }
    result
}

/// Add an alarm to a calendar item.
fn add_item_alarm(item: &EKCalendarItem, alarm: &AlarmInfo) {
    let ek_alarm = create_ek_alarm(alarm);
    unsafe { item.addAlarm(&ek_alarm) };
}

/// Remove an alarm from a calendar item by index.
fn remove_item_alarm(item: &EKCalendarItem, index: usize) -> Result<()> {
    let alarms = unsafe { item.alarms() };
    let Some(alarms) = alarms else {
        return Err(EventKitError::ItemNotFound("No alarms on this item".into()));
    };
    if index >= alarms.len() {
        return Err(EventKitError::ItemNotFound(format!(
            "Alarm index {} out of range ({})",
            index,
            alarms.len()
        )));
    }
    let alarm = alarms.objectAtIndex(index);
    unsafe { item.removeAlarm(&alarm) };
    Ok(())
}

/// Clear all alarms from a calendar item.
fn clear_item_alarms(item: &EKCalendarItem) {
    unsafe { item.setAlarms(None) };
}

/// Read all recurrence rules from a calendar item.
fn get_item_recurrence_rules(item: &EKCalendarItem) -> Vec<RecurrenceRule> {
    let rules = unsafe { item.recurrenceRules() };
    let Some(rules) = rules else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for i in 0..rules.len() {
        let rule = rules.objectAtIndex(i);
        result.push(recurrence_rule_to_info(&rule));
    }
    result
}

/// Set a single recurrence rule on a calendar item (replaces any existing).
fn set_item_recurrence_rule(item: &EKCalendarItem, rule: &RecurrenceRule) {
    let ek_rule = create_ek_recurrence_rule(rule);
    unsafe {
        let rules = NSArray::from_retained_slice(&[ek_rule]);
        item.setRecurrenceRules(Some(&rules));
    }
}

/// Clear all recurrence rules from a calendar item.
fn clear_item_recurrence_rules(item: &EKCalendarItem) {
    unsafe { item.setRecurrenceRules(None) };
}

/// Set URL on a calendar item.
fn set_item_url(item: &EKCalendarItem, url: Option<&str>) {
    unsafe {
        let ns_url = url.map(|u| {
            let ns_str = NSString::from_str(u);
            objc2_foundation::NSURL::URLWithString(&ns_str).unwrap()
        });
        item.setURL(ns_url.as_deref());
    }
}

/// Read URL from a calendar item.
fn get_item_url(item: &EKCalendarItem) -> Option<String> {
    unsafe { item.URL() }.map(|u| u.absoluteString().unwrap().to_string())
}

// ============================================================================
// Type conversion helpers
// ============================================================================

// Helper to convert an EKRecurrenceRule to a RecurrenceRule
fn recurrence_rule_to_info(rule: &EKRecurrenceRule) -> RecurrenceRule {
    let frequency = unsafe { rule.frequency() };
    let frequency = match frequency {
        EKRecurrenceFrequency::Daily => RecurrenceFrequency::Daily,
        EKRecurrenceFrequency::Weekly => RecurrenceFrequency::Weekly,
        EKRecurrenceFrequency::Monthly => RecurrenceFrequency::Monthly,
        EKRecurrenceFrequency::Yearly => RecurrenceFrequency::Yearly,
        _ => RecurrenceFrequency::Daily,
    };

    let interval = unsafe { rule.interval() } as usize;

    let end = unsafe { rule.recurrenceEnd() }
        .map(|end| {
            let count = unsafe { end.occurrenceCount() };
            if count > 0 {
                RecurrenceEndCondition::AfterCount(count)
            } else if let Some(date) = unsafe { end.endDate() } {
                RecurrenceEndCondition::OnDate(nsdate_to_datetime(&date))
            } else {
                RecurrenceEndCondition::Never
            }
        })
        .unwrap_or(RecurrenceEndCondition::Never);

    let days_of_week = unsafe { rule.daysOfTheWeek() }.map(|days| {
        let mut result = Vec::new();
        for i in 0..days.len() {
            let day = days.objectAtIndex(i);
            let weekday = unsafe { day.dayOfTheWeek() };
            result.push(weekday.0 as u8);
        }
        result
    });

    let days_of_month = unsafe { rule.daysOfTheMonth() }.map(|days| {
        let mut result = Vec::new();
        for i in 0..days.len() {
            let num = days.objectAtIndex(i);
            result.push(num.intValue());
        }
        result
    });

    RecurrenceRule {
        frequency,
        interval,
        end,
        days_of_week,
        days_of_month,
    }
}

// Helper to create an EKRecurrenceRule from a RecurrenceRule
fn create_ek_recurrence_rule(rule: &RecurrenceRule) -> Retained<EKRecurrenceRule> {
    let frequency = match rule.frequency {
        RecurrenceFrequency::Daily => EKRecurrenceFrequency::Daily,
        RecurrenceFrequency::Weekly => EKRecurrenceFrequency::Weekly,
        RecurrenceFrequency::Monthly => EKRecurrenceFrequency::Monthly,
        RecurrenceFrequency::Yearly => EKRecurrenceFrequency::Yearly,
    };

    let end = match &rule.end {
        RecurrenceEndCondition::Never => None,
        RecurrenceEndCondition::AfterCount(count) => {
            Some(unsafe { EKRecurrenceEnd::recurrenceEndWithOccurrenceCount(*count) })
        }
        RecurrenceEndCondition::OnDate(date) => {
            let nsdate = datetime_to_nsdate(*date);
            Some(unsafe { EKRecurrenceEnd::recurrenceEndWithEndDate(&nsdate) })
        }
    };

    let days_of_week: Option<Vec<Retained<EKRecurrenceDayOfWeek>>> =
        rule.days_of_week.as_ref().map(|days| {
            days.iter()
                .map(|&d| {
                    let weekday = EKWeekday(d as isize);
                    unsafe { EKRecurrenceDayOfWeek::dayOfWeek(weekday) }
                })
                .collect()
        });

    let days_of_month: Option<Vec<Retained<NSNumber>>> = rule
        .days_of_month
        .as_ref()
        .map(|days| days.iter().map(|&d| NSNumber::new_i32(d)).collect());

    let days_of_week_arr = days_of_week
        .as_ref()
        .map(|v| NSArray::from_retained_slice(v));
    let days_of_month_arr = days_of_month
        .as_ref()
        .map(|v| NSArray::from_retained_slice(v));

    unsafe {
        use objc2::AnyThread;
        EKRecurrenceRule::initRecurrenceWithFrequency_interval_daysOfTheWeek_daysOfTheMonth_monthsOfTheYear_weeksOfTheYear_daysOfTheYear_setPositions_end(
            EKRecurrenceRule::alloc(),
            frequency,
            rule.interval as isize,
            days_of_week_arr.as_deref(),
            days_of_month_arr.as_deref(),
            None, // months of year
            None, // weeks of year
            None, // days of year
            None, // set positions
            end.as_deref(),
        )
    }
}

// Helper to convert an EKAlarm to an AlarmInfo
fn alarm_to_info(alarm: &EKAlarm) -> AlarmInfo {
    let relative_offset = unsafe { alarm.relativeOffset() };
    let absolute_date = unsafe { alarm.absoluteDate() }.map(|d| nsdate_to_datetime(&d));

    let proximity = unsafe { alarm.proximity() };
    let proximity = match proximity {
        EKAlarmProximity::Enter => AlarmProximity::Enter,
        EKAlarmProximity::Leave => AlarmProximity::Leave,
        _ => AlarmProximity::None,
    };

    let location = unsafe { alarm.structuredLocation() }.map(|loc| {
        let title = unsafe { loc.title() }
            .map(|t| t.to_string())
            .unwrap_or_default();
        let radius = unsafe { loc.radius() };
        let (latitude, longitude) = unsafe { loc.geoLocation() }
            .map(|geo| {
                let coord = unsafe { geo.coordinate() };
                (coord.latitude, coord.longitude)
            })
            .unwrap_or((0.0, 0.0));

        StructuredLocation {
            title,
            latitude,
            longitude,
            radius,
        }
    });

    AlarmInfo {
        // relativeOffset of 0 means "at time of event" — it's always set
        relative_offset: Some(relative_offset),
        absolute_date,
        proximity,
        location,
    }
}

// Helper to create an EKAlarm from an AlarmInfo
fn create_ek_alarm(info: &AlarmInfo) -> Retained<EKAlarm> {
    let alarm = if let Some(date) = &info.absolute_date {
        let nsdate = datetime_to_nsdate(*date);
        unsafe { EKAlarm::alarmWithAbsoluteDate(&nsdate) }
    } else {
        let offset = info.relative_offset.unwrap_or(0.0);
        unsafe { EKAlarm::alarmWithRelativeOffset(offset) }
    };

    // Set proximity
    let prox = match info.proximity {
        AlarmProximity::Enter => EKAlarmProximity::Enter,
        AlarmProximity::Leave => EKAlarmProximity::Leave,
        AlarmProximity::None => EKAlarmProximity::None,
    };
    unsafe { alarm.setProximity(prox) };

    // Set structured location if provided
    if let Some(loc) = &info.location {
        let title = NSString::from_str(&loc.title);
        let structured = unsafe { EKStructuredLocation::locationWithTitle(&title) };
        unsafe { structured.setRadius(loc.radius) };

        // Create CLLocation for the geo coordinates
        #[cfg(feature = "location")]
        {
            use objc2::AnyThread;
            use objc2_core_location::CLLocation;
            let cl_location = unsafe {
                CLLocation::initWithLatitude_longitude(
                    CLLocation::alloc(),
                    loc.latitude,
                    loc.longitude,
                )
            };
            unsafe { structured.setGeoLocation(Some(&cl_location)) };
        }

        unsafe { alarm.setStructuredLocation(Some(&structured)) };
    }

    alarm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authorization_status_display() {
        assert_eq!(
            format!("{}", AuthorizationStatus::NotDetermined),
            "Not Determined"
        );
        assert_eq!(
            format!("{}", AuthorizationStatus::FullAccess),
            "Full Access"
        );
    }

    #[test]
    fn test_event_item_debug() {
        let event = EventItem {
            identifier: "test".to_string(),
            title: "Test Event".to_string(),
            notes: None,
            location: None,
            start_date: Local::now(),
            end_date: Local::now(),
            all_day: false,
            calendar_title: None,
            calendar_id: None,
            url: None,
            availability: EventAvailability::Busy,
            status: EventStatus::None,
            is_detached: false,
            occurrence_date: None,
            structured_location: None,
            attendees: Vec::new(),
            organizer: None,
        };
        assert!(format!("{:?}", event).contains("Test Event"));
    }
}

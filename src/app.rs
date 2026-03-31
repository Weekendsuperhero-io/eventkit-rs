use chrono::{Duration, Local, NaiveDateTime, TimeZone};
use clap::{Parser, Subcommand};
use eventkit::{AuthorizationStatus, EventKitError, EventsManager, RemindersManager};

#[derive(Parser)]
#[command(name = "eventkit")]
#[command(author, version, about = "Manage macOS Calendar and Reminders from the command line", long_about = None)]
struct Cli {
    /// Run as an MCP (Model Context Protocol) server over stdio
    #[cfg(feature = "mcp")]
    #[arg(long)]
    mcp: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Commands for managing reminders
    #[command(subcommand)]
    Reminders(RemindersCommands),

    /// Commands for managing calendar events
    #[command(subcommand)]
    Events(EventsCommands),

    /// Check authorization status
    Status {
        /// Check events status instead of reminders
        #[arg(short, long)]
        events: bool,
    },

    /// Dump raw objects as JSON for debugging
    #[cfg(feature = "mcp")]
    #[command(subcommand)]
    Dump(DumpCommands),
}

#[derive(Subcommand)]
enum RemindersCommands {
    /// Request authorization to access reminders
    Authorize,

    /// List all reminder lists (calendars)
    Lists,

    /// List reminders
    List {
        /// Filter by specific list(s)
        #[arg(short, long)]
        list: Option<Vec<String>>,

        /// Show only incomplete reminders
        #[arg(short, long)]
        incomplete: bool,

        /// Show completed reminders
        #[arg(short, long)]
        completed: bool,

        /// Show all details
        #[arg(short, long)]
        all: bool,

        /// Show all fields for debugging
        #[arg(long)]
        debug: bool,
    },

    /// Create a new reminder
    Add {
        /// Title of the reminder
        title: String,

        /// Notes/description for the reminder
        #[arg(short, long)]
        notes: Option<String>,

        /// List to add the reminder to
        #[arg(short, long)]
        list: Option<String>,

        /// Priority (0=none, 1-4=high, 5=medium, 6-9=low)
        #[arg(short, long)]
        priority: Option<usize>,
    },

    /// Update an existing reminder
    Update {
        /// Identifier of the reminder to update
        id: String,

        /// New title
        #[arg(short, long)]
        title: Option<String>,

        /// New notes
        #[arg(short, long)]
        notes: Option<String>,

        /// Priority (0=none, 1-4=high, 5=medium, 6-9=low)
        #[arg(short, long)]
        priority: Option<usize>,
    },

    /// Mark a reminder as complete
    Complete {
        /// Identifier of the reminder
        id: String,
    },

    /// Mark a reminder as incomplete
    Uncomplete {
        /// Identifier of the reminder
        id: String,
    },

    /// Delete a reminder
    Delete {
        /// Identifier of the reminder to delete
        id: String,

        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Show details of a specific reminder
    Show {
        /// Identifier of the reminder
        id: String,
    },
}

#[derive(Subcommand)]
enum EventsCommands {
    /// Request authorization to access calendar events
    Authorize,

    /// List all calendars
    Calendars,

    /// List events
    List {
        /// Show events for today only
        #[arg(short, long)]
        today: bool,

        /// Show events for the next N days (default: 7)
        #[arg(short, long, default_value = "7")]
        days: i64,

        /// Filter by specific calendar(s)
        #[arg(short, long)]
        calendar: Option<Vec<String>>,

        /// Show all details
        #[arg(short, long)]
        all: bool,
    },

    /// Create a new event
    Add {
        /// Title of the event
        title: String,

        /// Start date/time (format: YYYY-MM-DD HH:MM or YYYY-MM-DD for all-day)
        #[arg(short, long)]
        start: String,

        /// End date/time (format: YYYY-MM-DD HH:MM or YYYY-MM-DD for all-day)
        #[arg(short, long)]
        end: Option<String>,

        /// Duration in minutes (alternative to --end)
        #[arg(short, long, default_value = "60")]
        duration: i64,

        /// Notes/description
        #[arg(short, long)]
        notes: Option<String>,

        /// Location
        #[arg(short, long)]
        location: Option<String>,

        /// Calendar to add the event to
        #[arg(short, long)]
        calendar: Option<String>,

        /// Create as all-day event
        #[arg(long)]
        all_day: bool,
    },

    /// Delete an event
    Delete {
        /// Identifier of the event to delete
        id: String,

        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },

    /// Show details of a specific event
    Show {
        /// Identifier of the event
        id: String,
    },
}

#[cfg(feature = "mcp")]
#[derive(Subcommand)]
enum DumpCommands {
    /// Dump a single reminder with all fields, alarms, recurrence as JSON
    Reminder {
        /// Identifier of the reminder
        id: String,
    },
    /// Dump all reminders in a list (or all lists) as JSON
    Reminders {
        /// Filter to a specific list name
        #[arg(short, long)]
        list: Option<String>,
    },
    /// Dump a single event with all fields, alarms, recurrence, attendees as JSON
    Event {
        /// Identifier of the event
        id: String,
    },
    /// Dump events for the next N days as JSON
    Events {
        /// Number of days (default: 7)
        #[arg(short, long, default_value = "7")]
        days: i64,
    },
    /// Dump all reminder lists as JSON
    ReminderLists,
    /// Dump all event calendars as JSON
    Calendars,
    /// Dump all sources as JSON
    Sources,
}

pub fn run() {
    let cli = Cli::parse();

    // Handle --mcp flag
    #[cfg(feature = "mcp")]
    if cli.mcp {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        if let Err(e) = rt.block_on(eventkit::mcp::run_mcp_server()) {
            eprintln!("MCP server error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let Some(command) = cli.command else {
        // No subcommand and no --mcp flag: print help
        use clap::CommandFactory;
        Cli::command().print_help().ok();
        println!();
        std::process::exit(1);
    };

    let result = match command {
        Commands::Status { events } => cmd_status(events),
        #[cfg(feature = "mcp")]
        Commands::Dump(cmd) => cmd_dump(cmd),
        Commands::Reminders(cmd) => match cmd {
            RemindersCommands::Authorize => cmd_reminders_authorize(),
            RemindersCommands::Lists => cmd_reminders_lists(),
            RemindersCommands::List {
                list,
                incomplete,
                completed,
                all,
                debug,
            } => cmd_reminders_list(list, incomplete, completed, all, debug),
            RemindersCommands::Add {
                title,
                notes,
                list,
                priority,
            } => cmd_reminders_add(&title, notes.as_deref(), list.as_deref(), priority),
            RemindersCommands::Update {
                id,
                title,
                notes,
                priority,
            } => cmd_reminders_update(&id, title.as_deref(), notes.as_deref(), priority),
            RemindersCommands::Complete { id } => cmd_reminders_complete(&id),
            RemindersCommands::Uncomplete { id } => cmd_reminders_uncomplete(&id),
            RemindersCommands::Delete { id, force } => cmd_reminders_delete(&id, force),
            RemindersCommands::Show { id } => cmd_reminders_show(&id),
        },
        Commands::Events(cmd) => match cmd {
            EventsCommands::Authorize => cmd_events_authorize(),
            EventsCommands::Calendars => cmd_events_calendars(),
            EventsCommands::List {
                today,
                days,
                calendar,
                all,
            } => cmd_events_list(today, days, calendar, all),
            EventsCommands::Add {
                title,
                start,
                end,
                duration,
                notes,
                location,
                calendar,
                all_day,
            } => cmd_events_add(
                &title,
                &start,
                end.as_deref(),
                duration,
                notes.as_deref(),
                location.as_deref(),
                calendar.as_deref(),
                all_day,
            ),
            EventsCommands::Delete { id, force } => cmd_events_delete(&id, force),
            EventsCommands::Show { id } => cmd_events_show(&id),
        },
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ============================================================================
// Status command
// ============================================================================

fn cmd_status(events: bool) -> Result<(), EventKitError> {
    let (kind, status) = if events {
        ("Calendar Events", EventsManager::authorization_status())
    } else {
        ("Reminders", RemindersManager::authorization_status())
    };

    println!("{} Authorization Status: {}", kind, status);

    match status {
        AuthorizationStatus::NotDetermined => {
            println!(
                "\nUse 'eventkit {} authorize' to request access.",
                if events { "events" } else { "reminders" }
            );
        }
        AuthorizationStatus::Denied => {
            println!("\nAccess was denied. Please enable access in:");
            println!(
                "System Settings > Privacy & Security > {}",
                if events { "Calendars" } else { "Reminders" }
            );
        }
        AuthorizationStatus::Restricted => {
            println!("\nAccess is restricted by system policy.");
        }
        AuthorizationStatus::FullAccess => {
            println!("\nFull access granted.");
        }
        AuthorizationStatus::WriteOnly => {
            println!("\nWrite-only access granted.");
        }
    }

    Ok(())
}

// ============================================================================
// Reminders commands
// ============================================================================

fn cmd_reminders_authorize() -> Result<(), EventKitError> {
    let manager = RemindersManager::new();

    println!("Requesting access to Reminders...");

    match manager.request_access() {
        Ok(true) => {
            println!("✓ Access granted!");
            Ok(())
        }
        Ok(false) => {
            println!("✗ Access denied.");
            println!("\nTo grant access, go to:");
            println!("System Settings > Privacy & Security > Reminders");
            Err(EventKitError::AuthorizationDenied)
        }
        Err(e) => {
            println!("✗ Failed to request access: {}", e);
            Err(e)
        }
    }
}

fn cmd_reminders_lists() -> Result<(), EventKitError> {
    let manager = RemindersManager::new();
    let calendars = manager.list_calendars()?;

    if calendars.is_empty() {
        println!("No reminder lists found.");
        return Ok(());
    }

    println!("Reminder Lists:\n");

    for cal in calendars {
        let source = cal.source.as_deref().unwrap_or("Unknown");
        let modifiable = if cal.allows_modifications {
            ""
        } else {
            " (read-only)"
        };
        println!("  • {} [{}]{}", cal.title, source, modifiable);
        println!("    ID: {}", cal.identifier);
    }

    if let Ok(default) = manager.default_calendar() {
        println!("\nDefault list: {}", default.title);
    }

    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn cmd_reminders_list(
    list_filter: Option<Vec<String>>,
    incomplete: bool,
    show_completed: bool,
    show_all: bool,
    debug: bool,
) -> Result<(), EventKitError> {
    let manager = RemindersManager::new();

    let reminders = if incomplete {
        manager.fetch_incomplete_reminders()?
    } else if let Some(ref lists) = list_filter {
        let list_refs: Vec<&str> = lists.iter().map(std::string::String::as_str).collect();
        manager.fetch_reminders(Some(&list_refs))?
    } else {
        manager.fetch_all_reminders()?
    };

    let reminders: Vec<_> = if !incomplete && !show_completed && !show_all {
        reminders.into_iter().filter(|r| !r.completed).collect()
    } else if show_completed && !show_all {
        reminders.into_iter().filter(|r| r.completed).collect()
    } else {
        reminders
    };

    if reminders.is_empty() {
        println!("No reminders found.");
        return Ok(());
    }

    println!("Reminders ({}):\n", reminders.len());

    for reminder in reminders {
        let status = if reminder.completed { "✓" } else { "○" };
        let priority_str = match reminder.priority {
            0 => String::new(),
            1..=4 => " !!!".to_string(),
            5 => " !!".to_string(),
            _ => " !".to_string(),
        };

        println!("  {} {}{}", status, reminder.title, priority_str);

        if show_all {
            if let Some(ref notes) = reminder.notes {
                let truncated: String = notes.chars().take(60).collect();
                let suffix = if notes.len() > 60 { "..." } else { "" };
                println!("      Notes: {}{}", truncated, suffix);
            }
            if let Some(ref cal) = reminder.calendar_title {
                println!("      List: {}", cal);
            }
            println!("      ID: {}", reminder.identifier);
        }

        if debug {
            // Print all available fields for debugging
            println!("      Completed: {}", reminder.completed);
            println!("      Priority: {}", reminder.priority);

            if let Some(due_date) = reminder.due_date {
                println!("      Due Date: {}", due_date.format("%Y-%m-%d %H:%M:%S"));
            } else {
                println!("      Due Date: None");
            }

            if let Some(start_date) = reminder.start_date {
                println!(
                    "      Start Date: {}",
                    start_date.format("%Y-%m-%d %H:%M:%S")
                );
            } else {
                println!("      Start Date: None");
            }

            if let Some(completion_date) = reminder.completion_date {
                println!(
                    "      Completion Date: {}",
                    completion_date.format("%Y-%m-%d %H:%M:%S")
                );
            } else {
                println!("      Completion Date: None");
            }

            // Additional inherited fields from EKCalendarItem parent class
            if let Some(ref notes) = reminder.notes {
                println!("      Notes: {}", notes);
            }
            println!("      Has Notes: {}", reminder.has_notes);
            if let Some(ref cal) = reminder.calendar_title {
                println!("      Calendar/List: {}", cal);
            }
            if let Some(ref ext_id) = reminder.external_identifier {
                println!("      External ID: {}", ext_id);
            }
            if let Some(ref location) = reminder.location {
                println!("      Location: {}", location);
            }
            if let Some(ref url) = reminder.url {
                println!("      URL: {}", url);
            }
            if let Some(creation_date) = reminder.creation_date {
                println!(
                    "      Creation Date: {}",
                    creation_date.format("%Y-%m-%d %H:%M:%S")
                );
            } else {
                println!("      Creation Date: None");
            }
            if let Some(last_modified_date) = reminder.last_modified_date {
                println!(
                    "      Last Modified Date: {}",
                    last_modified_date.format("%Y-%m-%d %H:%M:%S")
                );
            } else {
                println!("      Last Modified Date: None");
            }
            if let Some(ref timezone) = reminder.timezone {
                println!("      Timezone: {}", timezone);
            }
            println!("      Has Alarms: {}", reminder.has_alarms);
            println!(
                "      Has Recurrence Rules: {}",
                reminder.has_recurrence_rules
            );
            println!("      Has Attendees: {}", reminder.has_attendees);
        }
    }

    if !show_all {
        println!("\nUse --all to see more details.");
    }

    Ok(())
}

fn cmd_reminders_add(
    title: &str,
    notes: Option<&str>,
    list: Option<&str>,
    priority: Option<usize>,
) -> Result<(), EventKitError> {
    if let Some(p) = priority
        && p > 9
    {
        eprintln!("Priority must be between 0 and 9");
        return Err(EventKitError::SaveFailed(
            "Invalid priority value".to_string(),
        ));
    }

    let manager = RemindersManager::new();
    let reminder = manager.create_reminder(title, notes, list, priority, None, None)?;

    println!("✓ Created reminder: {}", reminder.title);
    println!("  ID: {}", reminder.identifier);
    if let Some(cal) = reminder.calendar_title {
        println!("  List: {}", cal);
    }

    Ok(())
}

fn cmd_reminders_update(
    id: &str,
    title: Option<&str>,
    notes: Option<&str>,
    priority: Option<usize>,
) -> Result<(), EventKitError> {
    if title.is_none() && notes.is_none() && priority.is_none() {
        eprintln!("No updates specified. Use --title, --notes, or --priority.");
        return Ok(());
    }

    if let Some(p) = priority
        && p > 9
    {
        eprintln!("Priority must be between 0 and 9");
        return Err(EventKitError::SaveFailed(
            "Invalid priority value".to_string(),
        ));
    }

    let manager = RemindersManager::new();
    let reminder = manager.update_reminder(id, title, notes, None, priority, None, None, None)?;

    println!("✓ Updated reminder: {}", reminder.title);

    Ok(())
}

fn cmd_reminders_complete(id: &str) -> Result<(), EventKitError> {
    let manager = RemindersManager::new();
    let reminder = manager.complete_reminder(id)?;
    println!("✓ Completed: {}", reminder.title);
    Ok(())
}

fn cmd_reminders_uncomplete(id: &str) -> Result<(), EventKitError> {
    let manager = RemindersManager::new();
    let reminder = manager.uncomplete_reminder(id)?;
    println!("○ Marked incomplete: {}", reminder.title);
    Ok(())
}

fn cmd_reminders_delete(id: &str, force: bool) -> Result<(), EventKitError> {
    let manager = RemindersManager::new();
    let reminder = manager.get_reminder(id)?;

    if !force {
        println!("Delete reminder: \"{}\"?", reminder.title);
        println!("This action cannot be undone. Use --force to skip this prompt.");
        return Ok(());
    }

    manager.delete_reminder(id)?;
    println!("✓ Deleted: {}", reminder.title);

    Ok(())
}

fn cmd_reminders_show(id: &str) -> Result<(), EventKitError> {
    let manager = RemindersManager::new();
    let reminder = manager.get_reminder(id)?;

    println!("Reminder Details:\n");
    println!("  Title:     {}", reminder.title);
    println!(
        "  Status:    {}",
        if reminder.completed {
            "Completed"
        } else {
            "Incomplete"
        }
    );
    println!(
        "  Priority:  {}",
        match reminder.priority {
            0 => "None".to_string(),
            1..=4 => format!("High ({})", reminder.priority),
            5 => "Medium".to_string(),
            _ => format!("Low ({})", reminder.priority),
        }
    );

    if let Some(ref notes) = reminder.notes {
        println!("  Notes:     {}", notes);
    }

    if let Some(ref cal) = reminder.calendar_title {
        println!("  List:      {}", cal);
    }

    println!("  ID:        {}", reminder.identifier);

    Ok(())
}

// ============================================================================
// Events commands
// ============================================================================

fn cmd_events_authorize() -> Result<(), EventKitError> {
    let manager = EventsManager::new();

    println!("Requesting access to Calendar...");

    match manager.request_access() {
        Ok(true) => {
            println!("✓ Access granted!");
            Ok(())
        }
        Ok(false) => {
            println!("✗ Access denied.");
            println!("\nTo grant access, go to:");
            println!("System Settings > Privacy & Security > Calendars");
            Err(EventKitError::AuthorizationDenied)
        }
        Err(e) => {
            println!("✗ Failed to request access: {}", e);
            Err(e)
        }
    }
}

fn cmd_events_calendars() -> Result<(), EventKitError> {
    let manager = EventsManager::new();
    let calendars = manager.list_calendars()?;

    if calendars.is_empty() {
        println!("No calendars found.");
        return Ok(());
    }

    println!("Calendars:\n");

    for cal in calendars {
        let source = cal.source.as_deref().unwrap_or("Unknown");
        let modifiable = if cal.allows_modifications {
            ""
        } else {
            " (read-only)"
        };
        println!("  • {} [{}]{}", cal.title, source, modifiable);
        println!("    ID: {}", cal.identifier);
    }

    if let Ok(default) = manager.default_calendar() {
        println!("\nDefault calendar: {}", default.title);
    }

    Ok(())
}

#[allow(clippy::needless_pass_by_value)]
fn cmd_events_list(
    today: bool,
    days: i64,
    calendar_filter: Option<Vec<String>>,
    show_all: bool,
) -> Result<(), EventKitError> {
    let manager = EventsManager::new();

    let events = if today {
        manager.fetch_today_events()?
    } else if let Some(ref cals) = calendar_filter {
        let cal_refs: Vec<&str> = cals.iter().map(std::string::String::as_str).collect();
        let now = Local::now();
        let end = now + Duration::days(days);
        manager.fetch_events(now, end, Some(&cal_refs))?
    } else {
        manager.fetch_upcoming_events(days)?
    };

    if events.is_empty() {
        println!("No events found.");
        return Ok(());
    }

    println!("Events ({}):\n", events.len());

    let mut current_date = String::new();
    for event in events {
        let event_date = event.start_date.format("%Y-%m-%d").to_string();
        if event_date != current_date {
            current_date = event_date.clone();
            println!("\n  📅 {}", event.start_date.format("%A, %B %d, %Y"));
        }

        let time_str = if event.all_day {
            "All day".to_string()
        } else {
            format!(
                "{} - {}",
                event.start_date.format("%H:%M"),
                event.end_date.format("%H:%M")
            )
        };

        println!("     {} {}", time_str, event.title);

        if show_all {
            if let Some(ref location) = event.location {
                println!("        📍 {}", location);
            }
            if let Some(ref notes) = event.notes {
                let truncated: String = notes.chars().take(50).collect();
                let suffix = if notes.len() > 50 { "..." } else { "" };
                println!("        📝 {}{}", truncated, suffix);
            }
            if let Some(ref cal) = event.calendar_title {
                println!("        🗂  {}", cal);
            }
            println!("        ID: {}", event.identifier);
        }
    }

    if !show_all {
        println!("\nUse --all to see more details.");
    }

    Ok(())
}

fn parse_datetime(s: &str) -> Option<chrono::DateTime<Local>> {
    // Try "YYYY-MM-DD HH:MM" format first
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M") {
        return Local.from_local_datetime(&dt).single();
    }

    // Try "YYYY-MM-DD" format (for all-day events)
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0)?;
        return Local.from_local_datetime(&dt).single();
    }

    None
}

#[allow(clippy::too_many_arguments)]
fn cmd_events_add(
    title: &str,
    start_str: &str,
    end_str: Option<&str>,
    duration_mins: i64,
    notes: Option<&str>,
    location: Option<&str>,
    calendar: Option<&str>,
    all_day: bool,
) -> Result<(), EventKitError> {
    let start = parse_datetime(start_str).ok_or_else(|| {
        EventKitError::SaveFailed(
            "Invalid start date format. Use YYYY-MM-DD HH:MM or YYYY-MM-DD".to_string(),
        )
    })?;

    let end = if let Some(end_s) = end_str {
        parse_datetime(end_s).ok_or_else(|| {
            EventKitError::SaveFailed(
                "Invalid end date format. Use YYYY-MM-DD HH:MM or YYYY-MM-DD".to_string(),
            )
        })?
    } else if all_day {
        start + Duration::days(1)
    } else {
        start + Duration::minutes(duration_mins)
    };

    let manager = EventsManager::new();
    let event = manager.create_event(title, start, end, notes, location, calendar, all_day)?;

    println!("✓ Created event: {}", event.title);
    println!("  Start: {}", event.start_date.format("%Y-%m-%d %H:%M"));
    println!("  End:   {}", event.end_date.format("%Y-%m-%d %H:%M"));
    println!("  ID: {}", event.identifier);
    if let Some(cal) = event.calendar_title {
        println!("  Calendar: {}", cal);
    }

    Ok(())
}

fn cmd_events_delete(id: &str, force: bool) -> Result<(), EventKitError> {
    let manager = EventsManager::new();
    let event = manager.get_event(id)?;

    if !force {
        println!("Delete event: \"{}\"?", event.title);
        println!("This action cannot be undone. Use --force to skip this prompt.");
        return Ok(());
    }

    manager.delete_event(id, false)?;
    println!("✓ Deleted: {}", event.title);

    Ok(())
}

#[cfg(feature = "mcp")]
fn cmd_dump(cmd: DumpCommands) -> Result<(), EventKitError> {
    let json = match cmd {
        DumpCommands::Reminder { id } => eventkit::mcp::dump_reminder(&id)?,
        DumpCommands::Reminders { list } => eventkit::mcp::dump_reminders(list.as_deref())?,
        DumpCommands::Event { id } => eventkit::mcp::dump_event(&id)?,
        DumpCommands::Events { days } => eventkit::mcp::dump_events(days)?,
        DumpCommands::ReminderLists => eventkit::mcp::dump_reminder_lists()?,
        DumpCommands::Calendars => eventkit::mcp::dump_calendars()?,
        DumpCommands::Sources => eventkit::mcp::dump_sources()?,
    };
    println!("{json}");
    Ok(())
}

fn cmd_events_show(id: &str) -> Result<(), EventKitError> {
    let manager = EventsManager::new();
    let event = manager.get_event(id)?;

    println!("Event Details:\n");
    println!("  Title:     {}", event.title);
    println!("  Start:     {}", event.start_date.format("%Y-%m-%d %H:%M"));
    println!("  End:       {}", event.end_date.format("%Y-%m-%d %H:%M"));
    println!("  All Day:   {}", if event.all_day { "Yes" } else { "No" });

    if let Some(ref location) = event.location {
        println!("  Location:  {}", location);
    }

    if let Some(ref notes) = event.notes {
        println!("  Notes:     {}", notes);
    }

    if let Some(ref cal) = event.calendar_title {
        println!("  Calendar:  {}", cal);
    }

    println!("  ID:        {}", event.identifier);

    Ok(())
}

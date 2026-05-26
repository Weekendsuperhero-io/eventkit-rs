# eventkit-rs

[![Crates.io](https://img.shields.io/crates/v/eventkit-rs.svg)](https://crates.io/crates/eventkit-rs)
[![Documentation](https://docs.rs/eventkit-rs/badge.svg)](https://docs.rs/eventkit-rs)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![CI](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml)

A Rust library and CLI for interacting with macOS Calendar events and Reminders via Apple's EventKit framework. Includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server for AI assistant integration.

## Features

- **Calendar Events** — full CRUD with alarms, recurrence (incl. `monthsOfTheYear` / `setPositions`), attendees, location chip, URL, structured location, availability (Busy/Free/Tentative/Unavailable), calendar move, and `EKSpan` choice (`this` vs `future`) for recurring edits/deletes.
- **Reminders** — full CRUD with priority, alarms (time + geofence), recurrence (including `monthsOfTheYear` / `setPositions` etc.), due/start dates, due-date IANA timezone, and explicit completion-date setting.
- **MCP Server** — built-in MCP server (`--mcp`) with structured JSON output, typed input/output schemas, and **32 tools**.
- **Calendar Management** — create, update (name + color), and delete both reminder lists and event calendars.
- **Batch Operations** — batch delete, move, and update across reminders and events.
- **Authorization** — programmatic consent prompts for Reminders, Calendar, and Location; status diagnostics with remediation hints.
- **Location** — get current location via CoreLocation; auto-prompt for location auth when a geofence is attached.
- **In-Process Transport** — embed as a library with `serve_on()` for `DuplexStream`-based in-process MCP.
- **CLI + Dump + Reflection** — command-line tool with curated JSON dumps and runtime ObjC reflection dumps (`dump reminder-raw`, `dump reminder-private`) for debugging.

> **iCloud caveats** (verified empirically — see project notes). On iCloud-synced reminder lists, the iCloud daemon silently drops some `EKReminder` writes even when the framework call succeeds: the plain-text `location`, `EKReminder.structuredLocation`, `EKAlarm.url`/`soundName`/`emailAddress`. These setters exist in the lib (parity with EventKit) for non-iCloud sources, but the MCP/CLI surface for **reminders** is intentionally pruned of those fields. **Calendar events** are unaffected — their URL/location/etc. work end-to-end. For "remind me at a place" on iCloud, use a **geofence** (location-based alarm) — that's the iCloud-honored path.

## Requirements

- macOS 14+ (Sonoma)
- Rust 1.94+

## Installation

### As a CLI Tool

```bash
cargo install eventkit-rs
```

### As a Library

```toml
[dependencies]
eventkit-rs = "0.5"
```

Without MCP dependencies:

```toml
[dependencies]
eventkit-rs = { version = "0.5", default-features = false, features = ["events", "reminders"] }
```

## Quick Start

### Library Usage

```rust
use chrono::{Duration, Local};
use eventkit::{EventsManager, ReminderDraft, ReminderPatch, RemindersManager};

fn main() -> eventkit::Result<()> {
    // Reminders ----------------------------------------------------------
    let reminders = RemindersManager::new();
    reminders.request_access()?;

    let created = reminders.create_reminder(&ReminderDraft {
        title: "Buy groceries",
        notes: Some("Milk, eggs, bread"),
        priority: Some(1),
        due_date: Some(Local::now() + Duration::days(1)),
        due_date_timezone: Some("America/Los_Angeles"),
        ..Default::default()
    })?;
    println!("created: {}", created.identifier);

    // List incomplete items due before next week
    let upcoming = reminders.fetch_incomplete_reminders_in_due_range(
        None,
        Some(Local::now() + Duration::days(7)),
        None,
    )?;
    for r in upcoming {
        println!("- {}", r.title);
    }

    // Backdate completion
    reminders.update_reminder(
        &created.identifier,
        &ReminderPatch {
            completion_date: Some(Some(Local::now() - Duration::hours(2))),
            ..Default::default()
        },
    )?;

    // Calendar Events ----------------------------------------------------
    let events = EventsManager::new();
    events.request_access()?;
    for event in events.fetch_today_events()? {
        println!("{} at {}", event.title, event.start_date);
    }
    Ok(())
}
```

### In-Process MCP (no separate binary)

```rust
use tokio::io::duplex;

let (client_stream, server_stream) = duplex(64 * 1024);

tokio::spawn(async move {
    eventkit::mcp::serve_on(server_stream).await.unwrap();
});

// Connect your MCP client to client_stream...
```

### CLI Usage

```bash
# MCP server (stdio transport)
eventkit --mcp

# Authorization
eventkit status                              # reminders status
eventkit status --events                     # events status
eventkit reminders authorize                 # trigger consent dialog
eventkit events authorize

# Reminders — basics
eventkit reminders lists
eventkit reminders list --all
eventkit reminders add "Call mom" --notes "Birthday" --priority 1 --due-tz America/Los_Angeles
eventkit reminders show <id>
eventkit reminders update <id> --title "Call mom back" --priority 1
eventkit reminders update <id> --completion-date "2026-05-20 10:00"   # backdate complete
eventkit reminders update <id> --completion-date ""                   # mark incomplete
eventkit reminders complete <id>
eventkit reminders uncomplete <id>
eventkit reminders delete <id> --force

# Reminders — date-range queries (uses EventKit's native predicates)
eventkit reminders list --due-after 2026-05-25 --due-before 2026-06-01
eventkit reminders list --completed-after 2026-05-19 --completed-before 2026-05-21

# Reminders — posture setters
eventkit reminders set-due-tz <id> "America/Los_Angeles"
eventkit reminders set-geofence <id> --loc-title "Home" --lat 37.78 --lng -122.42 --radius 100 --proximity enter
eventkit reminders clear-geofence <id>
eventkit reminders set-URL <id> "https://example.com"   # diagnostic; iCloud may ignore in UI

# Calendar Events (URL + location are first-class on events)
eventkit events calendars
eventkit events list --today
eventkit events list --days 14 --all
eventkit events add "Meeting" --start "2026-03-22 14:00" --duration 60 \
    --url https://example.com/zoom --availability tentative
eventkit events add "Holiday" --start "2026-03-25" --all-day --location "Office"
eventkit events show <id>

# Calendar Events — update + recurring-event span semantics
eventkit events update <id> --location "Conf Room 4" --availability free
eventkit events update <id> --calendar "Work"                 # move calendars
eventkit events update <id> --all-day true                    # flip to all-day
eventkit events update <id> --title "Standup (moved)" --span future   # this + future
eventkit events update <id> --notes ""                        # clear notes
eventkit events delete <id> --force                           # delete this occurrence
eventkit events delete <id> --force --span future             # delete this + future

# Dump objects as JSON or raw ObjC reflection (debugging)
eventkit dump reminder-lists
eventkit dump calendars
eventkit dump sources
eventkit dump reminder <id>                  # curated JSON
eventkit dump reminders --list "Shopping"
eventkit dump reminder-raw <id>              # every @property on the EKReminder class chain
eventkit dump reminder-raw <id> --values     # ...with KVC value reads
eventkit dump reminder-private <id>          # probe non-public/private selectors
eventkit dump event <id>
eventkit dump events --days 30
```

## MCP Server

All tools return structured JSON with typed output schemas. Responses use `structured_content` (MCP spec 2025-06-18) with text fallback for older clients.

### Tools (32)

| Tool | Description |
|---|---|
| **Authorization** ||
| `auth_status` | Check Reminders + Calendar permission status without prompting; returns remediation hints. |
| `request_access` | Trigger the OS consent dialog for `entity: "reminder" \| "event"`. |
| **Reminder Lists** ||
| `list_reminder_lists` | List all reminder lists with color, source, permissions. |
| `create_reminder_list` | Create a new reminder list. |
| `update_reminder_list` | Update name and/or color (red, blue, green, purple, etc.). |
| `delete_reminder_list` | Delete a list and all its reminders. |
| **Reminders** ||
| `list_reminders` | List reminders. Filters: `show_completed`, `list_name`, `due_after`/`due_before` (incomplete), `completed_after`/`completed_before` (completed). |
| `create_reminder` | Create with inline alarms, recurrence, due/start dates, due-date IANA timezone, geofence. |
| `update_reminder` | Update any of the above plus `completion_date` (authoritative completion toggle). |
| `get_reminder` | Get full detail (alarms, recurrence rules inline). |
| `delete_reminder` | Delete a reminder. |
| `complete_reminder` | Mark as completed. |
| `uncomplete_reminder` | Mark as not completed. |
| `set_reminder_due_timezone` | Set or clear the IANA timezone applied to the due date. |
| `set_reminder_geofence` | Attach (or clear) a location-based alarm — "remind me when I arrive/leave". Auto-prompts for Location permission. |
| **Event Calendars** ||
| `list_calendars` | List all event calendars with color, source, permissions. |
| `get_default_event_calendar` | Return the calendar `create_event` will use when `calendar_name` is omitted. |
| `create_event_calendar` | Create a new calendar. |
| `update_event_calendar` | Update name and/or color. |
| `delete_event_calendar` | Delete a calendar and all its events. |
| **Events** ||
| `list_events` | List events by date range, filter by calendar ID. |
| `create_event` | Create with inline alarms, recurrence, URL, location, `availability`, `structured_location`. |
| `update_event` | Update title, notes (clearable), location (clearable), start/end, `all_day`, calendar move (`calendar_name`), URL (clearable), `availability`, `structured_location`, alarms, recurrence. `span: "this" \| "future"` controls recurring-event edit scope. |
| `get_event` | Get full detail (alarms, recurrence, attendees, organizer, `creation_date`, `last_modified_date`, `external_identifier`, `timezone`, `attachments_count`). |
| `delete_event` | Delete one event or a recurring series. `span: "this" \| "future"` (legacy `affect_future: bool` still accepted as alias). |
| `set_event_availability` | Set an event's availability to `"busy" \| "free" \| "tentative" \| "unavailable"`. Always per-instance. |
| **Search & Location** ||
| `search` | Search reminders and/or events by text (`item_type` optional). |
| `get_current_location` | Get lat/long via CoreLocation. |
| `list_sources` | List accounts (iCloud, Local, Exchange). |
| **Batch** ||
| `batch_delete` | Delete multiple reminders or events at once. |
| `batch_move` | Move multiple reminders between lists. |
| `batch_update` | Update multiple items at once. |

### Prompts (4)

| Prompt | Description |
|---|---|
| `incomplete_reminders` | List all incomplete reminders (optionally by list). |
| `reminder_lists` | List all available reminder lists. |
| `move_reminder` | Move a reminder to a different list. |
| `create_detailed_reminder` | Create a reminder with notes, priority, and due date. |

### Configuration

Add to your MCP client config (e.g. Claude Desktop):

```json
{
  "mcpServers": {
    "eventkit": {
      "command": "eventkit",
      "args": ["--mcp"]
    }
  }
}
```

## Privacy Permissions

Embedded automatically when you use the CLI (see `Info.plist` in this repo). When linking as a library into your own app, add to your `Info.plist`:

```xml
<key>NSRemindersFullAccessUsageDescription</key>
<string>This app needs access to your reminders.</string>

<key>NSCalendarsFullAccessUsageDescription</key>
<string>This app needs access to your calendar.</string>

<key>NSLocationWhenInUseUsageDescription</key>
<string>This app needs your location for location-based reminders.</string>

<!-- Required for geofenced reminder triggers to fire when the app isn't foreground -->
<key>NSLocationAlwaysAndWhenInUseUsageDescription</key>
<string>This app needs background location access so location-based reminders trigger when you arrive at or leave a place.</string>
```

## Feature Flags

| Feature | Default | Description |
|---|---|---|
| `events` | Yes | Calendar event support |
| `reminders` | Yes | Reminders support |
| `location` | Yes | CoreLocation for geofenced reminders |
| `mcp` | Yes | MCP server, structured JSON output, dump commands |

## Development

```bash
# Run all checks (format + clippy + build + nextest) — same as CI
./ci-check.sh

# Auto-fix formatting + clippy, then check
./ci-check.sh --fix

# Run tests directly (parallel via nextest; live-eventkit tests run serial)
cargo nextest run --all-features

# Run only the live-EventKit integration tests (touches real authorization)
cargo nextest run --all-features --run-ignored only

# Build universal binary (arm64 + x86_64)
./build-universal.sh
```

## License

Apache 2.0 — see [LICENSE](LICENSE).

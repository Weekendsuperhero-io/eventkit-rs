# eventkit-rs

[![Crates.io](https://img.shields.io/crates/v/eventkit-rs.svg)](https://crates.io/crates/eventkit-rs)
[![Documentation](https://docs.rs/eventkit-rs/badge.svg)](https://docs.rs/eventkit-rs)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![CI](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml)

A Rust library and CLI for interacting with macOS Calendar events and Reminders via Apple's EventKit framework. Includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server for AI assistant integration.

## Features

- **Calendar Events** &mdash; Create, read, update, and delete calendar events with alarms, recurrence, attendees, and structured location
- **Reminders** &mdash; Full CRUD for reminders with priority, alarms, recurrence, tags, and URL support
- **MCP Server** &mdash; Built-in MCP server (`--mcp`) with structured JSON output, input/output schemas, and 26 tools
- **Calendar Management** &mdash; Create, update (name + color), and delete both reminder lists and event calendars
- **Batch Operations** &mdash; Batch delete, move, and update across reminders and events
- **Location** &mdash; Get current location via CoreLocation for geofenced reminders
- **In-Process Transport** &mdash; Embed as a library with `serve_on()` for DuplexStream-based in-process MCP
- **CLI + Dump** &mdash; Command-line tool with JSON dump for debugging raw EventKit objects

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
eventkit-rs = "0.3"
```

Without MCP dependencies:

```toml
[dependencies]
eventkit-rs = { version = "0.3", default-features = false, features = ["events", "reminders"] }
```

## Quick Start

### Library Usage

```rust
use eventkit::{RemindersManager, EventsManager};

fn main() -> eventkit::Result<()> {
    // Reminders
    let reminders = RemindersManager::new();
    reminders.request_access()?;

    let reminder = reminders.create_reminder(
        "Buy groceries",
        Some("Milk, eggs, bread"),
        None, Some(1), None, None,
    )?;

    for item in reminders.fetch_incomplete_reminders()? {
        println!("- {}", item.title);
    }

    // Calendar Events
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

// One end for the MCP client, one for the server
let (client_stream, server_stream) = duplex(64 * 1024);

// Spawn the EventKit MCP server in-process
tokio::spawn(async move {
    eventkit::mcp::serve_on(server_stream).await.unwrap();
});

// Connect your MCP client to client_stream...
```

### CLI Usage

```bash
# MCP server (stdio transport)
eventkit --mcp

# Reminders
eventkit reminders authorize
eventkit reminders lists
eventkit reminders list --all
eventkit reminders add "Call mom" --notes "Birthday" --priority 1
eventkit reminders complete <id>
eventkit reminders delete <id> --force

# Calendar Events
eventkit events authorize
eventkit events calendars
eventkit events list --today
eventkit events list --days 14 --all
eventkit events add "Meeting" --start "2026-03-22 14:00" --duration 60
eventkit events add "Holiday" --start "2026-03-25" --all-day
eventkit events delete <id> --force

# Dump objects as JSON (for debugging)
eventkit dump reminder-lists
eventkit dump calendars
eventkit dump sources
eventkit dump reminder <id>
eventkit dump reminders --list "Shopping"
eventkit dump event <id>
eventkit dump events --days 30
```

## MCP Server

### Tools (26)

All tools return structured JSON with typed output schemas. Responses use `structured_content` (MCP spec 2025-06-18) with text fallback for older clients.

| Tool | Description |
|---|---|
| **Reminder Lists** ||
| `list_reminder_lists` | List all reminder lists with color, source, permissions |
| `create_reminder_list` | Create a new reminder list |
| `update_reminder_list` | Update name and/or color (red, blue, green, purple, etc.) |
| `delete_reminder_list` | Delete a list and all its reminders |
| **Reminders** ||
| `list_reminders` | List reminders, filter by list and completion status |
| `create_reminder` | Create with inline alarms, recurrence, URL, tags, priority |
| `update_reminder` | Update any fields including alarms, recurrence, URL, tags |
| `get_reminder` | Get full detail (alarms, recurrence, tags inline) |
| `delete_reminder` | Delete a reminder |
| `complete_reminder` | Mark as completed |
| `uncomplete_reminder` | Mark as not completed |
| **Event Calendars** ||
| `list_calendars` | List all event calendars with color, source, permissions |
| `create_event_calendar` | Create a new calendar |
| `update_event_calendar` | Update name and/or color |
| `delete_event_calendar` | Delete a calendar and all its events |
| **Events** ||
| `list_events` | List events by date range, filter by calendar ID |
| `create_event` | Create with inline alarms, recurrence, URL |
| `update_event` | Update any fields including alarms, recurrence, URL |
| `get_event` | Get full detail (alarms, recurrence, attendees, organizer) |
| `delete_event` | Delete (with `affect_future` for recurring events) |
| **Search & Location** ||
| `search` | Search reminders and/or events by text (item_type optional) |
| `get_current_location` | Get lat/long via CoreLocation |
| `list_sources` | List accounts (iCloud, Local, Exchange) |
| **Batch** ||
| `batch_delete` | Delete multiple reminders or events at once |
| `batch_move` | Move multiple reminders between lists |
| `batch_update` | Update multiple items at once |

### Prompts (4)

| Prompt | Description |
|---|---|
| `incomplete_reminders` | List all incomplete reminders (optionally by list) |
| `reminder_lists` | List all available reminder lists |
| `move_reminder` | Move a reminder to a different list |
| `create_detailed_reminder` | Create a reminder with notes, priority, and due date |

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

Add to your `Info.plist`:

```xml
<key>NSRemindersFullAccessUsageDescription</key>
<string>This app needs access to your reminders.</string>

<key>NSCalendarsFullAccessUsageDescription</key>
<string>This app needs access to your calendar.</string>

<key>NSLocationWhenInUseUsageDescription</key>
<string>This app needs your location for location-based reminders.</string>
```

## Feature Flags

| Feature | Default | Description |
|---|---|---|
| `events` | Yes | Calendar event support |
| `reminders` | Yes | Reminders support |
| `location` | Yes | CoreLocation for geofenced reminders |
| `mcp` | Yes | MCP server, structured JSON output, dump command |

## Development

```bash
# Run all checks (same as CI)
./ci-check.sh

# Auto-fix formatting + clippy
./ci-check.sh --fix

# Build universal binary (arm64 + x86_64)
./build-universal.sh
```

## License

Apache 2.0 &mdash; see [LICENSE](LICENSE).

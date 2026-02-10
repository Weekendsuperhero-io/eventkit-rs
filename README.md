# eventkit-rs

[![Crates.io](https://img.shields.io/crates/v/eventkit-rs.svg)](https://crates.io/crates/eventkit-rs)
[![Documentation](https://docs.rs/eventkit-rs/badge.svg)](https://docs.rs/eventkit-rs)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![CI](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/weekendsuperhero/eventkit-rs/actions/workflows/ci.yml)

A Rust library and CLI for interacting with macOS Calendar events and Reminders via Apple's EventKit framework. Includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server for AI assistant integration.

## Features

- 📅 **Calendar Events**: Create, read, update, and delete calendar events
- ✅ **Reminders**: Full CRUD operations for reminders/tasks
- 🤖 **MCP Server**: Built-in MCP server (`--mcp`) for AI assistant integration
- 🔐 **Authorization**: Safe handling of macOS privacy permissions
- 🦀 **Pure Rust**: Built on `objc2` for safe Objective-C interop
- 💻 **CLI Included**: Command-line tool for quick access

## Installation

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
eventkit-rs = "0.2"
```

To use without MCP dependencies:

```toml
[dependencies]
eventkit-rs = { version = "0.2", default-features = false, features = ["events", "reminders"] }
```

### As a CLI Tool

```bash
cargo install eventkit-rs
```

## Quick Start

### Library Usage

```rust
use eventkit::{RemindersManager, EventsManager, Result};

fn main() -> Result<()> {
    // === Working with Reminders ===
    let reminders = RemindersManager::new();

    // Request access (required on first use)
    reminders.request_access()?;

    // Create a reminder
    let reminder = reminders.create_reminder(
        "Buy groceries",
        Some("Milk, eggs, bread"),
        None,  // Use default list
        Some(1),  // High priority
    )?;
    println!("Created: {}", reminder.title);

    // List incomplete reminders
    for item in reminders.fetch_incomplete_reminders()? {
        println!("- {} ({})", item.title, if item.completed { "✓" } else { "○" });
    }

    // Complete a reminder
    reminders.complete_reminder(&reminder.identifier)?;

    // === Working with Calendar Events ===
    let events = EventsManager::new();
    events.request_access()?;

    // List today's events
    for event in events.fetch_today_events()? {
        println!("📅 {} at {}", event.title, event.start_date);
    }

    // Create an event
    use chrono::{Local, Duration};
    let start = Local::now() + Duration::hours(1);
    let end = start + Duration::hours(2);

    let event = events.create_event(
        "Team Meeting",
        start,
        end,
        Some("Discuss Q4 planning"),
        Some("Conference Room A"),
        None,  // Use default calendar
        false, // Not all-day
    )?;

    Ok(())
}
```

### CLI Usage

```bash
# Check authorization status
eventkit status
eventkit status --events

# === MCP Server ===

# Start as an MCP server (stdio transport)
eventkit --mcp

# === Reminders ===

# Request authorization
eventkit reminders authorize

# List reminder lists
eventkit reminders lists

# List incomplete reminders
eventkit reminders list

# List all reminders with details
eventkit reminders list --all

# Create a reminder
eventkit reminders add "Call mom" --notes "Birthday wishes" --priority 1

# Complete a reminder
eventkit reminders complete <id>

# Delete a reminder
eventkit reminders delete <id> --force

# === Calendar Events ===

# Request authorization
eventkit events authorize

# List calendars
eventkit events calendars

# List today's events
eventkit events list --today

# List next 14 days
eventkit events list --days 14

# List with full details
eventkit events list --all

# Create an event
eventkit events add "Team Meeting" \
    --start "2024-12-20 14:00" \
    --duration 60 \
    --location "Conference Room" \
    --notes "Q4 Planning"

# Create all-day event
eventkit events add "Company Holiday" \
    --start "2024-12-25" \
    --all-day

# Show event details
eventkit events show <id>

# Delete an event
eventkit events delete <id> --force
```

## Platform Support

This library only works on **macOS**. It requires:

- macOS 10.14 (Mojave) or later
- Rust 1.92 or later

## Privacy Permissions

Your application needs to request permission to access Calendar and/or Reminders data. Add these keys to your `Info.plist`:

```xml
<!-- For Reminders access -->
<key>NSRemindersUsageDescription</key>
<string>This app needs access to your reminders to help you manage tasks.</string>

<!-- For Calendar access (macOS 14+) -->
<key>NSCalendarsFullAccessUsageDescription</key>
<string>This app needs access to your calendar to manage events.</string>

<!-- For Calendar access (older macOS) -->
<key>NSCalendarsUsageDescription</key>
<string>This app needs access to your calendar to manage events.</string>
```

## API Reference

### RemindersManager

| Method                         | Description                  |
| ------------------------------ | ---------------------------- |
| `new()`                        | Create a new manager         |
| `authorization_status()`       | Check current auth status    |
| `request_access()`             | Request reminders permission |
| `list_calendars()`             | List all reminder lists      |
| `fetch_all_reminders()`        | Fetch all reminders          |
| `fetch_incomplete_reminders()` | Fetch incomplete reminders   |
| `fetch_reminders(calendars)`   | Fetch from specific lists    |
| `create_reminder(...)`         | Create a new reminder        |
| `update_reminder(...)`         | Update an existing reminder  |
| `complete_reminder(id)`        | Mark as complete             |
| `uncomplete_reminder(id)`      | Mark as incomplete           |
| `delete_reminder(id)`          | Delete a reminder            |

### EventsManager

| Method                                | Description                 |
| ------------------------------------- | --------------------------- |
| `new()`                               | Create a new manager        |
| `authorization_status()`              | Check current auth status   |
| `request_access()`                    | Request calendar permission |
| `list_calendars()`                    | List all calendars          |
| `fetch_today_events()`                | Fetch today's events        |
| `fetch_upcoming_events(days)`         | Fetch next N days           |
| `fetch_events(start, end, calendars)` | Fetch in date range         |
| `create_event(...)`                   | Create a new event          |
| `update_event(...)`                   | Update an existing event    |
| `delete_event(id)`                    | Delete an event             |

### MCP Server

The `mcp` module (enabled by default) provides a full MCP server:

| Function / Type    | Description                                  |
| ------------------ | -------------------------------------------- |
| `run_mcp_server()` | Start the MCP server on stdio transport      |
| `EventKitServer`   | The MCP server struct implementing all tools |

#### MCP Tools

| Tool                   | Description                  |
| ---------------------- | ---------------------------- |
| `reminders_authorize`  | Request Reminders access     |
| `list_reminder_lists`  | List all reminder lists      |
| `list_reminders`       | List reminders (filterable)  |
| `create_reminder`      | Create a new reminder        |
| `update_reminder`      | Update an existing reminder  |
| `complete_reminder`    | Mark a reminder as completed |
| `delete_reminder`      | Delete a reminder            |
| `create_reminder_list` | Create a new reminder list   |
| `rename_reminder_list` | Rename a reminder list       |
| `delete_reminder_list` | Delete a reminder list       |
| `events_authorize`     | Request Calendar access      |
| `list_calendars`       | List all calendars           |
| `list_events`          | List events (by day range)   |
| `create_event`         | Create a new calendar event  |
| `delete_event`         | Delete a calendar event      |

#### MCP Configuration

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

## Feature Flags

| Feature     | Default | Description                              |
| ----------- | ------- | ---------------------------------------- |
| `events`    | Yes     | Calendar event support                   |
| `reminders` | Yes     | Reminders support                        |
| `mcp`       | Yes     | MCP server (`--mcp` flag + `mcp` module) |

## Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [objc2](https://github.com/madsmtm/objc2) for Objective-C interop
- Inspired by the need for native Rust access to macOS productivity apps

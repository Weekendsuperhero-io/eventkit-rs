# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **PR description workflow** — introduced a new GitHub Actions workflow to automatically update PR descriptions on open and edit events.

### Changed
- **Cross-platform compilation** — moved `objc2`/`EventKit` dependencies behind `cfg(target_os = "macos")` and split source into platform-gated modules so the crate compiles as an empty shell on non-macOS platforms.
- **Dependencies** — updated `tokio` to 1.51, bumped `rmcp` to 1.3 for the latest MCP protocol features, and updated Cargo lockfile.
- **Project cleanup** — reorganized `Cargo.toml` metadata and features, replaced explicit closures with function pointers, and added clippy annotations for better maintainability.
- **Tracing logs** — implemented lighter logging for tracing.
- **CI/CD workflows** — updated the workflow strategy for universal builds.

### Fixed
- **Schema generation** — resolved stderr noise during schema generation by explicitly mapping Rust types (`usize`, `u8`) to standard schemars types.
- **CI/CD workflows** — fixed GitHub Actions workflow and release job yaml configurations.

## [0.2.0] - 2025-02-10

### Added

- **MCP Server**: Built-in Model Context Protocol (MCP) server via `--mcp` flag
  - Exposes all Calendar and Reminders functionality as MCP tools
  - Runs over stdio transport for easy integration with AI assistants
  - Gated behind the `mcp` feature (enabled by default)
- `mcp` module with `EventKitServer` and `run_mcp_server()` public API

### Changed

- `mcp` feature is now included in default features (`events`, `reminders`, `mcp`)
- CLI `command` field is now optional to support the top-level `--mcp` flag

### Fixed

- Event save/remove operations now use the explicit `commit: true` variants
  - `saveEvent:span:error:` replaced with `saveEvent:span:commit:error:` (commit = true)
  - `removeEvent:span:error:` replaced with `removeEvent:span:commit:error:` (commit = true)
  - Ensures events are committed to the Calendar database immediately, consistent with how reminders and calendars were already handled

## [0.1.0] - 2024-XX-XX

### Added

- Initial release
- `RemindersManager` for full CRUD operations on macOS Reminders
  - Create, read, update, delete reminders
  - List reminder calendars (lists)
  - Mark reminders complete/incomplete
  - Filter by calendar and completion status
- `EventsManager` for calendar event management
  - Create, read, update, delete calendar events
  - Fetch events by date range
  - Support for all-day events
  - List calendars
- Authorization handling for both reminders and calendar access
- CLI tool (`eventkit`) with subcommands:
  - `eventkit reminders` - Manage reminders
  - `eventkit events` - Manage calendar events
  - `eventkit status` - Check authorization status
- Comprehensive documentation
- GitHub Actions for CI/CD
- MIT License

### Known Limitations

- macOS only (10.14+)
- Recurring events show as individual occurrences
- No support for event invitations/attendees management

[Unreleased]: https://github.com/weekendsuperhero/eventkit-rs/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/weekendsuperhero/eventkit-rs/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/weekendsuperhero/eventkit-rs/releases/tag/v0.1.0

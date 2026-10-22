---
created: 2026-05-29T04:27
updated: 2026-10-16T00:00
updated: 2026-10-13T00:00
updated: 2026-10-14T00:00
---
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **The server names itself** — `serverInfo` was rmcp's own (`rmcp 3.6.0`,
  `rmcp 2.2.0` before), since `get_info` never set it and
  `Implementation::from_build_env()` reads rmcp's crate. It is now `calendar`,
  titled "Calendar & Reminders", at this crate's version.
- **EventKit store exhaustion, again** — 0.6.0 cached the store per thread, and the MCP server ran its tools on every tokio worker, with two stores each (reminders and events): twenty on a ten-core Mac, each a connection to the calendar daemon, and every authorization change reset them all. Agent Muse's beta was refused twice (2026-10-14 and 2026-10-15) with `EKCADErrorDomain 1021 — "This process has too many EKEventStore instances. Use fewer event stores."`
  - Every tool call and prompt now runs on one dedicated thread (`mcp_thread.rs`, named `eventkit`), so the server holds one store however many connections or workers its host has. rmcp's service loop stays on the host's runtime, so a call that waits on the consent dialog no longer gets a host's liveness pings timed out.
  - Reminders and events share that one store, as Apple's single instance does. The cache remembers each kind's authorization status, and a change in either resets the store.
  - A store EventKit refuses is dropped, and the next call builds a new one. It used to stay cached and fail every call on its thread until a restart.
  - Prompts too. `#[prompt_handler]` replaces a hand-written `get_prompt` with its own (rmcp-macros 2.x and 3.x alike), so the first cut's thread-routed `get_prompt` never ran: every prompt, each of which builds a `RemindersManager`, still opened a store on whichever worker served it. The handler now writes `get_prompt` and `list_prompts` itself, and `get_prompt_is_ours_not_the_macros` keeps the attribute off.
### Added
- **Calendars and reminders as resources** (`src/mcp_resources.rs`), read-only
  JSON with the tools' own fields plus each listed item's `uri`:
  `calendar://calendars`, `calendar://events{?from,to}`,
  `calendar://calendars/{calendarId}/events{?from,to}`,
  `calendar://events/{eventId}`, `reminders://lists`, `reminders://reminders`,
  `reminders://lists/{listId}{?completed}` and
  `reminders://reminders/{reminderId}`. A window defaults to today and the six
  days after and spans at most a year; identifiers are one percent-encoded
  segment. Listing never asks for access: the four fixed resources always, one
  per calendar and per list only once full access is granted. A 2026-07-28
  client is told a read and the list stay fresh for a minute (`Private`), the
  templates an hour (`Public`). An unknown URI or id is `-32002`, a bad
  parameter `-32602`. Listing and reading run on the EventKit thread.
- `RemindersManager::fetch_list_reminders(list_id, include_completed)`: one
  list's reminders by identifier (a title can name two lists), refusing an id
  that is not a reminder list's.
- **MCP tasks, as the SEP-2663 extension** (`io.modelcontextprotocol/tasks`,
  offered on both revisions), version 0.8.0 — a call to `list_reminders`,
  `list_events`, `search`, `batch_delete`, `batch_move`, `batch_update`,
  `request_access` or `get_current_location` from a client that declared the
  extension becomes a task when it is still running after 1 second: the client
  gets a task handle at once and polls `tasks/get`. Quick calls, and every call
  from a client that did not declare the extension, are answered inline as
  before. Up to 4 tasks run at once (each takes its turn on the one EventKit
  thread, where every EventKit call runs) and 256 are held; a task lives
  1 hour. A tool that panics fails its task instead of stranding it. Closing
  the connection asks every task to stop; one still running after 2 seconds is
  left to finish the item it is on and stop, never aborted mid-item. A task
  past its hour is aborted, with its batch told to stop first.
- **Batches stop between items when cancelled** — `batch_delete`,
  `batch_move` and `batch_update` check for cancellation (`tasks/cancel`,
  `notifications/cancelled`, or the connection closing) before each item. A
  cancelled batch task is `cancelled`, with a status saying how far it got;
  while it runs, its status and `_meta["io.agentmuse/progress"]` on `tasks/get`
  report items done out of the total. A single EventKit call cannot stop
  part-way: a task cancelled during one is `completed`.

### Changed
- **Rust 1.99 or newer** (`rust-version`), the toolchain Agent Muse builds with.
- **`rmcp` 2.2.0 → 3.6.0**, exact-pinned to the embedding Agent Muse
  workspace's copy, so the app links one rmcp.
- **MCP 2026-07-28 alongside 2025-11-25** — a 2026-07-28 client runs that
  revision's stateless lifecycle: `server/discover` answers with every
  revision this server implements (the list a `-32022` carries too), every
  request carries its own
  version, identity and capabilities in `_meta`, and results carry
  `resultType`. A 2025-era client still opens with `initialize`, answered in
  2025-11-25 exactly as before, also when it asks for 2026-07-28.
  `server/discover` at a 2025 version is method-not-found, so no client runs
  the stateless lifecycle at a revision that defines none, and a revision
  newer than 2026-07-28 is refused `-32022` with the list. The ceiling is named
  (`MAX_PROTOCOL_VERSION`), never rmcp's `LATEST`.

### Removed
- **MCP tasks (SEP-1686), version 0.7.0** — the server no longer advertises
  `capabilities.tasks`; `list_reminders`, `list_events`, `search`,
  `batch_delete`, `batch_move` and `batch_update` no longer declare
  `execution.taskSupport`; and `tasks/list`, `tasks/get`, `tasks/result` and
  `tasks/cancel` are gone, with the `notifications/tasks/status` push and the
  task manager behind them (`mcp_tasks.rs`). Agent Muse, which embeds this
  server, measured the task surface unused (no task-augmented call, no
  `tasks/*` request in two weeks of traffic), and every rmcp 3.x release
  removes the MCP 2025-11-25 task model. A task-augmented `tools/call` runs as
  a plain call, the spec's rule for a server without the capability. EventKit
  calls stay synchronous on the
  handler's own thread. The `tokio-util` and `uuid` dependencies went with
  it. Tasks returned in 0.8.0 as the SEP-2663 extension (see Added), with
  `tokio-util` for the operation tokens. A breaking change, hence the minor
  version bump.

## [0.6.0] - 2026-08-04

Versions 0.3.0 through 0.5.x shipped without changelog entries; their changes are
folded in here rather than reconstructed after the fact.

### Fixed
- **EventKit store exhaustion (critical)** — `RemindersManager::new()` and `EventsManager::new()` each constructed a fresh `EKEventStore` on *every* call, and the MCP layer builds a manager per tool call. A busy session allocated one store per call until the calendar daemon refused the process outright:
  `EKCADErrorDomain 1021 — "This process has too many EKEventStore instances. Use fewer event stores."`
  Stores are now cached **per thread** (`StoreCache`), so the process holds two rather than one per call. Apple: *"instantiate and use a single instance of `EKEventStore` … an `EKEventStore` object requires a significant amount of time to initialize and release."* A test asserts there is exactly one construction site so this cannot regress silently.
- **Reads silently returned empty under write-only access** — `WriteOnly` was treated as authorized, so `ensure_authorized()` returned `Ok` and every read succeeded with *nothing in it*. Apple returns no events at all to a write-only client, so an agent would confidently report an empty calendar to a user who simply hadn't granted full access. Authorization is now split into `ensure_full_access` / `ensure_write_access`, with a distinct `AuthorizationWriteOnly` error naming the right remedy.
- **Store went stale after a mid-session grant** — caching the store required the other half of Apple's contract: the store is now `reset()` on an authorization *transition*, so granting access in System Settings takes effect without an app restart. Not called when the status is unchanged, since `reset()` discards uncommitted changes.
- **Consent dialog could be suppressed entirely** — an `eventStoreIdentifier()` call on the construction path aborted the thread on any machine that had not yet granted access (`objc2` declares the accessor non-null; EventKit returns NULL when unauthorized). The abort landed before `ensure_full_access`, so access was never requested and no TCC prompt ever appeared. Manager construction is now inert — it must never call an EventKit accessor.
- **`auth_status` misreported write-only as granted** — the same write-only bug at the MCP layer; only `FullAccess` counts as granted.
- **A panicking tool permanently disabled background tasks** — a task is marked terminal only *after* `call_tool` returns, so an unwind stranded it in `Working`: TTL pruning skipped it and it held an active slot forever. After 16 such panics, *every* subsequent task was refused for the life of the process, and `tasks/result` waiters blocked on a slot nothing would fill. A drop guard now marks the task `Failed` and publishes a result during the unwind.
- **A cancelled task could be downgraded** — a worker finishing just after `tasks/cancel` overwrote `Cancelled` with `Completed`/`Failed` and pushed a second status notification. Terminal is now final.
- **An EventKit completion block that never fired wedged a worker thread forever** — all four blocking bridges (both `request_access` paths and both reminder fetches) waited on a condvar with no timeout. On a host where the consent dialog can never be answered — a headless process with no TCC grant — `requestFullAccessTo*` stayed outstanding and the thread was removed from the tokio worker pool permanently, with the server simply going silent. Waits are now bounded: 30s for data calls, 120s for authorization (which legitimately waits on a human), each reporting a distinct timeout error.
- **Poisoned locks could brick a manager** — the blocking bridges that wait on Obj-C completion blocks moved from `std::sync::Mutex` to `parking_lot`, which does not poison. A panic inside a completion block previously made every later `lock()` panic, turning one failed call into a permanently dead manager. Also removed eight `.unwrap()`s.
- **Schema generation** — resolved stderr noise by explicitly mapping `usize` / `u8` to standard schemars types.
- **CI/CD workflows** — fixed GitHub Actions workflow and release job yaml configurations.

### Added
- **MCP task surface (SEP-1686)** — slow tools (full-calendar scans, batch operations) can be invoked task-augmented, returning a task id immediately. Bounded memory (16 concurrent, 256 tracked, 1h TTL), real cancellation, and a pushed `notifications/tasks/status` on every transition so clients need not poll.
- **MCP Server** — `auth_status` and `request_access` tools, tools to set due timezone, geofence, and event availability, and an embedded `Info.plist` to trigger macOS TCC privacy prompts.
- **EventKit Core** — exposed `URL`, `availability`, `structured_location`, `due_date_timezone`, and `attachments_count`, plus raw reflection via `dump_reminder_raw` / `dump_reminder_private`.
- **Testing** — live EventKit tests (including store-cache checks against external writes) and MCP smoke tests. The memory- and liveness-critical paths are covered directly rather than incidentally: completion-block timeouts, task TTL pruning and oldest-terminal eviction, panic containment, and a single-store-construction-site assertion.
- **PR description workflow** — GitHub Actions workflow to update PR descriptions on open and edit.

### Changed
- **Dependencies** — `rmcp` to 2.2 (from the 1.3/1.4 line), `tokio` to 1.51.
- **EventKit Core** — creation and updates refactored onto `Draft` and `Patch` structs.
- **Error messages** — authorization errors now carry TCC remediation steps.
- **Cross-platform compilation** — `objc2`/EventKit gated behind `cfg(target_os = "macos")`, with platform-split modules so the crate compiles as an empty shell elsewhere.
- **CI & Infrastructure** — testing migrated from `cargo test` to `cargo nextest`; universal build workflow strategy updated.
- **Tracing logs** — lighter logging.
- **Documentation** — refreshed `README.md`; added `AUTHORIZATION_PLAN.md` recording the authorization/caching work and its incident history.
- **Project cleanup** — reorganized `Cargo.toml` metadata and features, replaced explicit closures with function pointers, clippy annotations.

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

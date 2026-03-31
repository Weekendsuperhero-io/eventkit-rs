//! # EventKit-RS
//!
//! A Rust library for interacting with macOS Calendar and Reminders via EventKit.
//!
//! This library provides safe wrappers around the Apple EventKit framework to:
//! - Request and check authorization for calendar and reminders access
//! - List, create, update, and delete calendar events
//! - List, create, update, and delete reminders
//! - Manage calendars and reminder lists
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use eventkit::{RemindersManager, EventsManager, Result};
//!
//! fn main() -> Result<()> {
//!     // Working with reminders
//!     let reminders = RemindersManager::new();
//!     reminders.request_access()?;
//!
//!     for reminder in reminders.fetch_incomplete_reminders()? {
//!         println!("Todo: {}", reminder.title);
//!     }
//!
//!     // Working with calendar events
//!     let events = EventsManager::new();
//!     events.request_access()?;
//!
//!     for event in events.fetch_today_events()? {
//!         println!("Event: {}", event.title);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Platform Support
//!
//! This library only works on macOS. It requires macOS 10.14 or later for full functionality.
//!
//! ## Privacy Permissions
//!
//! Your application will need to request calendar and/or reminders permissions.
//! Make sure to include the appropriate keys in your `Info.plist`:
//!
//! - `NSRemindersUsageDescription` - for reminders access
//! - `NSCalendarsFullAccessUsageDescription` - for calendar access (macOS 14+)
//! - `NSCalendarsUsageDescription` - for calendar access (older macOS)

// This entire crate is macOS-only (EventKit framework). On other platforms it
// compiles as an empty shell so that `cargo build --workspace` works everywhere.
// Dependents gate their eventkit usage with `#[cfg(target_os = "macos")]`.

#[cfg(target_os = "macos")]
mod imp;
#[cfg(target_os = "macos")]
pub use imp::*;

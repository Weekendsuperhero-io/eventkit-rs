//! # EventKit CLI
//!
//! A command-line interface for managing macOS Calendar events and Reminders.

#[cfg(target_os = "macos")]
mod app;

fn main() {
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("eventkit requires macOS");
        std::process::exit(1);
    }

    #[cfg(target_os = "macos")]
    app::run();
}

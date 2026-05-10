//! # EventKit CLI
//!
//! A command-line interface for managing macOS Calendar events and Reminders.

// Embed Info.plist into the binary's __TEXT,__info_plist section so macOS
// can find the privacy usage descriptions when EventKit requests access.
// Without this, requestFullAccessTo{Reminders,Events} fails silently with no
// dialog and the binary has no stable TCC identity across rebuilds.
#[cfg(target_os = "macos")]
embed_plist::embed_info_plist!("../Info.plist");

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

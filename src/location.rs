//! CoreLocation integration for getting the user's current location.
//!
//! Uses `CLLocationManager` to request a one-shot location fix.
//! Requires the `location` feature and `NSLocationWhenInUseUsageDescription` in Info.plist.

use crate::{EventKitError, Result};
use objc2::rc::Retained;
use objc2_core_location::{CLLocationCoordinate2D, CLLocationManager};
use std::time::Duration;

/// Authorization status for location services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationAuthorizationStatus {
    NotDetermined,
    Restricted,
    Denied,
    Authorized,
}

/// A location coordinate (latitude, longitude).
#[derive(Debug, Clone, Copy)]
pub struct Coordinate {
    pub latitude: f64,
    pub longitude: f64,
}

/// Manages CoreLocation access for getting the user's current location.
///
/// CLLocationManager is `!Send + !Sync` — create instances on the thread
/// where you need them (same pattern as EventKit managers).
pub struct LocationManager {
    manager: Retained<CLLocationManager>,
}

impl LocationManager {
    /// Create a new LocationManager.
    pub fn new() -> Self {
        let manager = unsafe { CLLocationManager::new() };
        Self { manager }
    }

    /// Check the current authorization status for location services.
    pub fn authorization_status(&self) -> LocationAuthorizationStatus {
        let status = unsafe { self.manager.authorizationStatus() };
        // CLAuthorizationStatus is a newtype around i32:
        // 0 = notDetermined, 1 = restricted, 2 = denied,
        // 3 = authorizedAlways, 4 = authorizedWhenInUse
        match status.0 {
            0 => LocationAuthorizationStatus::NotDetermined,
            1 => LocationAuthorizationStatus::Restricted,
            2 => LocationAuthorizationStatus::Denied,
            3 | 4 => LocationAuthorizationStatus::Authorized,
            _ => LocationAuthorizationStatus::Denied,
        }
    }

    /// Request when-in-use authorization for location services.
    ///
    /// On macOS, this shows the system permission dialog. The result is
    /// available via `authorization_status()` after the user responds.
    pub fn request_when_in_use_authorization(&self) {
        unsafe {
            self.manager.requestWhenInUseAuthorization();
        }
    }

    /// Block until the location authorization status is resolved (the user
    /// dismisses the permission dialog) or `timeout` elapses.
    ///
    /// `requestWhenInUseAuthorization` only *presents* the dialog; the user's
    /// choice is delivered asynchronously on the thread's run loop, and the
    /// cached status read by `authorization_status()` doesn't change until that
    /// callback is serviced. EventKit's own access requests use a completion
    /// block + condvar, but `CLLocationManager` has no completion-block API — it
    /// reports back through its delegate on the run loop. So we pump the current
    /// thread's run loop (the handlers run off the main thread, where nothing is
    /// pumping it) and re-read the status until it leaves `NotDetermined`.
    ///
    /// Returns the resolved status; still `NotDetermined` if the user never
    /// answered before the timeout.
    pub fn wait_for_authorization(&self, timeout: Duration) -> LocationAuthorizationStatus {
        use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop};

        let start = std::time::Instant::now();
        loop {
            let status = self.authorization_status();
            if status != LocationAuthorizationStatus::NotDetermined {
                return status;
            }
            if start.elapsed() >= timeout {
                return status;
            }
            // Run the run loop for up to 100ms so CoreLocation can deliver the
            // authorization-change callback. `runMode:beforeDate:` returns early
            // if an input source fires, so this is responsive yet not a busy loop.
            unsafe {
                let until = NSDate::dateWithTimeIntervalSinceNow(0.1);
                NSRunLoop::currentRunLoop().runMode_beforeDate(NSDefaultRunLoopMode, &until);
            }
        }
    }

    /// Get the most recently cached location without starting updates.
    ///
    /// Returns `None` if no location has been determined yet.
    pub fn cached_location(&self) -> Option<Coordinate> {
        let location = unsafe { self.manager.location() }?;
        let coord: CLLocationCoordinate2D = unsafe { location.coordinate() };
        Some(Coordinate {
            latitude: coord.latitude,
            longitude: coord.longitude,
        })
    }

    /// Request a fresh location fix synchronously (blocks until result or timeout).
    ///
    /// Starts location updates, waits for a result, then stops updates.
    /// Times out after `timeout` duration.
    pub fn get_current_location(&self, timeout: Duration) -> Result<Coordinate> {
        match self.authorization_status() {
            LocationAuthorizationStatus::NotDetermined => {
                self.request_when_in_use_authorization();
                // Wait for the user to actually answer the permission dialog
                // rather than bailing while it's still on screen.
                match self.wait_for_authorization(Duration::from_secs(60)) {
                    LocationAuthorizationStatus::Authorized => {}
                    LocationAuthorizationStatus::Restricted => {
                        return Err(EventKitError::AuthorizationRestricted);
                    }
                    // Denied, or still NotDetermined because the user ignored
                    // the dialog until it timed out.
                    _ => return Err(EventKitError::AuthorizationDenied),
                }
            }
            LocationAuthorizationStatus::Denied => {
                return Err(EventKitError::AuthorizationDenied);
            }
            LocationAuthorizationStatus::Restricted => {
                return Err(EventKitError::AuthorizationRestricted);
            }
            LocationAuthorizationStatus::Authorized => {}
        }

        // Start location updates and poll for a result
        unsafe {
            self.manager.startUpdatingLocation();
        }

        // Poll for location (CLLocationManager updates are delivered on the
        // run loop; we poll the cached location property)
        let start = std::time::Instant::now();
        let result = loop {
            if let Some(location) = self.cached_location() {
                break Some(location);
            }

            if start.elapsed() >= timeout {
                break None;
            }

            // Brief sleep to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(100));
        };

        unsafe {
            self.manager.stopUpdatingLocation();
        }

        result.ok_or_else(|| EventKitError::EventKitError("Location request timed out".into()))
    }
}

impl Default for LocationManager {
    fn default() -> Self {
        Self::new()
    }
}

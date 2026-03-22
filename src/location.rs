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
                // Give the system a moment to process the authorization request
                std::thread::sleep(Duration::from_millis(500));
                if self.authorization_status() != LocationAuthorizationStatus::Authorized {
                    return Err(EventKitError::AuthorizationDenied);
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

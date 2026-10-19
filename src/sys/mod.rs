//! Platform abstraction for system data collection.
//!
//! On Linux and macOS this module is a plain re-export of `psutil` and
//! `battery`, so behaviour there is unchanged. FreeBSD is not supported by
//! either crate, so it gets native implementations based on `sysctl`,
//! `getfsstat`, `getifaddrs` and friends, exposed through the same API.

#[cfg(not(target_os = "freebsd"))]
pub use ::battery;
#[cfg(not(target_os = "freebsd"))]
pub use ::psutil::{cpu, disk, host, memory, network, process};
// The temperature widget reads the sensors through `sysinfo` on macOS, so the psutil module is
// only re-exported where it is actually used.
#[cfg(not(any(target_os = "freebsd", target_os = "macos")))]
pub use ::psutil::sensors;

#[cfg(target_os = "freebsd")]
pub mod freebsd;
#[cfg(target_os = "freebsd")]
pub use freebsd::{battery, cpu, disk, host, memory, network, process, sensors};

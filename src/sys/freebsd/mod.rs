//! Native FreeBSD implementations of the system data interfaces.
//!
//! Neither `psutil` nor `battery` support FreeBSD, so the data needed by the widgets is collected
//! directly from the kernel with `sysctlbyname(3)`, `getfsstat(2)`, `getifaddrs(3)` and
//! `gethostname(3)`. The exposed API mirrors the parts of the two crates that the widgets use.

pub mod battery;
pub mod cpu;
pub mod disk;
pub mod host;
pub mod memory;
pub mod network;
pub mod process;
pub mod sensors;
pub mod sysctl;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

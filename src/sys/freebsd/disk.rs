//! Partitions and disk usage from `getfsstat(2)` and `statfs(2)`.

use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::sysctl;
use super::Result;

/// Filesystems that are not backed by a disk.
const PSEUDO_FILESYSTEMS: &[&str] = &[
	"autofs",
	"devfs",
	"fdesc",
	"fdescfs",
	"linprocfs",
	"linsysfs",
	"mntfs",
	"nullfs",
	"procfs",
	"tmpfs",
];

pub struct Partition {
	device: String,
	mountpoint: PathBuf,
}

impl Partition {
	pub fn device(&self) -> &str {
		&self.device
	}

	pub fn mountpoint(&self) -> &Path {
		&self.mountpoint
	}
}

/// Disk usage of the filesystem containing a path, using the same formulas as psutil.
#[derive(Default)]
pub struct DiskUsage {
	free: u64,
	percent: f32,
}

impl DiskUsage {
	pub fn free(&self) -> u64 {
		self.free
	}

	pub fn percent(&self) -> f32 {
		self.percent
	}
}

/// Number of bytes read and written on a disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskIoCounters {
	read_count: u64,
	write_count: u64,
}

impl DiskIoCounters {
	pub fn read_count(&self) -> u64 {
		self.read_count
	}

	pub fn write_count(&self) -> u64 {
		self.write_count
	}
}

#[derive(Default)]
pub struct DiskIoCountersCollector {}

impl DiskIoCountersCollector {
	/// Per disk io counters are not implemented on FreeBSD: the counters live in `struct
	/// devstat`, which the `libc` crate does not declare, and parsing the kernel's copy by hand
	/// would depend on its layout. The columns are zeroed like they are on macOS.
	pub fn disk_io_counters_per_partition(&self) -> Result<HashMap<String, DiskIoCounters>> {
		Ok(HashMap::new())
	}
}

/// All mounted filesystems except the ones that are not backed by a disk.
pub fn partitions_physical() -> Result<Vec<Partition>> {
	// SAFETY: querying the number of mounts with a NULL buffer is what `getmntinfo(3)` does as
	// well, and a negative return value is turned into an error below.
	let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
	if count < 0 {
		return Err(io::Error::last_os_error().into());
	}

	let mut mounts = vec![unsafe { std::mem::zeroed::<libc::statfs>() }; count as usize];
	// SAFETY: `mounts` has room for `count` entries of the size the kernel expects.
	let written = unsafe {
		libc::getfsstat(
			mounts.as_mut_ptr(),
			(mounts.len() * std::mem::size_of::<libc::statfs>()) as libc::c_long,
			libc::MNT_NOWAIT,
		)
	};
	if written < 0 {
		return Err(io::Error::last_os_error().into());
	}
	mounts.truncate(written as usize);

	Ok(mounts
		.into_iter()
		.filter_map(|mount| {
			let filesystem = sysctl::fixed_string(&mount.f_fstypename);
			let device = sysctl::fixed_string(&mount.f_mntfromname);
			let mountpoint = sysctl::fixed_string(&mount.f_mntonname);
			// Partitions without a device name have no disk behind them and would break the
			// widgets, which take the disk name from the device.
			if filesystem.is_empty()
				|| device.is_empty()
				|| mountpoint.is_empty()
				|| Path::new(&device).file_name().is_none()
				|| PSEUDO_FILESYSTEMS.contains(&filesystem.as_str())
			{
				return None;
			}
			Some(Partition {
				device,
				mountpoint: PathBuf::from(mountpoint),
			})
		})
		.collect())
}

/// Disk usage of the filesystem containing the path.
pub fn disk_usage<P: AsRef<Path>>(path: P) -> Result<DiskUsage> {
	let path = CString::new(path.as_ref().as_os_str().as_bytes())
		.map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
	let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
	// SAFETY: `stat` is a writable output buffer of the size `statfs(2)` expects.
	if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
		return Err(io::Error::last_os_error().into());
	}

	// FreeBSD counts `f_blocks` in units of `f_bsize`, which is what `df(1)` uses as well.
	let block_size = stat.f_bsize;
	let total = stat.f_blocks.saturating_mul(block_size);
	// `f_bavail` is what non root users may use, `f_bfree` additionally includes the blocks
	// reserved for root.
	let free = (stat.f_bavail.max(0) as u64).saturating_mul(block_size);
	let used = total.saturating_sub(stat.f_bfree.saturating_mul(block_size));
	let percent = match used.saturating_add(free) {
		0 => 0.0,
		used_and_free => (used as f64 / used_and_free as f64 * 100.0) as f32,
	};

	Ok(DiskUsage { free, percent })
}

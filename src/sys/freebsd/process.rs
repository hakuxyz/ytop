//! Process list from the `kern.proc.all` sysctl node.

use std::collections::HashMap;
use std::mem::{offset_of, size_of};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::memory::VirtualMemory;
use super::sysctl;
use super::Result;

pub type Pid = u32;
pub type ProcessError = super::Error;
pub type ProcessResult<T> = std::result::Result<T, ProcessError>;

/// Set once so that an unsupported `struct kinfo_proc` layout is only reported once.
static UNSUPPORTED_LAYOUT_WARNED: AtomicBool = AtomicBool::new(false);

pub struct ProcessCollector {
	pub processes: HashMap<Pid, Process>,
	page_size: u64,
}

impl ProcessCollector {
	pub fn new() -> Result<ProcessCollector> {
		let mut collector = ProcessCollector {
			processes: HashMap::new(),
			page_size: sysctl::number("hw.pagesize")?,
		};
		// The first reading only collects the cpu times, so the cpu percent of the first update
		// is measured against the startup of ytop.
		collector.update()?;
		Ok(collector)
	}

	pub fn update(&mut self) -> Result<()> {
		let now = Instant::now();
		let previous: HashMap<Pid, (f64, Instant)> = self
			.processes
			.iter()
			.map(|(pid, process)| (*pid, (process.cpu_seconds, process.sampled_at)))
			.collect();

		let mut processes = HashMap::new();
		for info in read_processes(self.page_size)? {
			let cpu_seconds = info.cpu_seconds;
			let (previous_seconds, previous_at) = previous
				.get(&info.pid)
				.copied()
				// A process that just appeared has no previous reading to compare against.
				.unwrap_or((cpu_seconds, now));
			let wall = now.duration_since(previous_at).as_secs_f64();
			let cpu_percent = if wall > 0.0 && cpu_seconds >= previous_seconds {
				((cpu_seconds - previous_seconds) / wall * 100.0) as f32
			} else {
				0.0
			};

			processes.insert(
				info.pid,
				Process {
					pid: info.pid,
					name: info.name,
					rss_bytes: info.rss_bytes,
					cpu_seconds,
					cpu_percent,
					sampled_at: now,
				},
			);
		}
		self.processes = processes;
		Ok(())
	}
}

pub struct Process {
	pid: Pid,
	name: String,
	rss_bytes: u64,
	cpu_seconds: f64,
	cpu_percent: f32,
	sampled_at: Instant,
}

impl Process {
	pub fn pid(&self) -> Pid {
		self.pid
	}

	pub fn name(&self) -> String {
		self.name.clone()
	}

	/// The percentage of a single CPU the process used since the last update.
	pub fn cpu_percent(&self) -> f32 {
		self.cpu_percent
	}

	/// The percentage of the system memory the process occupies, like psutil computes it.
	pub fn memory_percent_oneshot(&self, virtual_memory: &VirtualMemory) -> ProcessResult<f32> {
		if virtual_memory.total() == 0 {
			return Ok(0.0);
		}
		Ok((self.rss_bytes as f64 / virtual_memory.total() as f64 * 100.0) as f32)
	}
}

struct ProcessInfo {
	pid: Pid,
	name: String,
	rss_bytes: u64,
	cpu_seconds: f64,
}

fn read_processes(page_size: u64) -> Result<Vec<ProcessInfo>> {
	let buffer = sysctl::bytes("kern.proc.all")?;
	if buffer.is_empty() {
		return Ok(Vec::new());
	}

	// The offsets of the fields ytop needs have been the same since FreeBSD 12; only the size of
	// the struct grew, and the kernel reports its own size in `ki_structsize`.
	let stride = read::<i32>(&buffer, offset_of!(libc::kinfo_proc, ki_structsize));
	let offset_pid = offset_of!(libc::kinfo_proc, ki_pid);
	let offset_name = offset_of!(libc::kinfo_proc, ki_comm);
	let offset_rss = offset_of!(libc::kinfo_proc, ki_rssize);
	let offset_rusage = offset_of!(libc::kinfo_proc, ki_rusage);

	if stride <= 0
		|| buffer.len() % stride as usize != 0
		|| offset_name + libc::COMMLEN + 1 > stride as usize
		|| offset_rusage + size_of::<libc::rusage>() > stride as usize
	{
		// FreeBSD 11 and older use a different layout, and an unknown future layout would be
		// just as wrong. The process widget stays empty instead of showing garbage.
		if !UNSUPPORTED_LAYOUT_WARNED.swap(true, Ordering::Relaxed) {
			log::warn!(
				"ytop: the kernel reported an unsupported struct kinfo_proc layout, \
				 the process widget stays empty"
			);
		}
		return Ok(Vec::new());
	}
	let stride = stride as usize;

	let mut processes = Vec::with_capacity(buffer.len() / stride);
	for entry in buffer.chunks_exact(stride) {
		let pid = read::<i32>(entry, offset_pid);
		if pid < 0 {
			continue;
		}
		let name = name(entry, offset_name);
		// Partly initialised or garbled entries have no usable name.
		if name.is_empty() || name.bytes().any(|byte| byte < 0x20 || byte == 0x7f) {
			continue;
		}
		let rss_pages = read::<libc::segsz_t>(entry, offset_rss).max(0);
		let rusage = read::<libc::rusage>(entry, offset_rusage);
		processes.push(ProcessInfo {
			pid: pid as Pid,
			name,
			rss_bytes: rss_pages as u64 * page_size,
			cpu_seconds: seconds(rusage.ru_utime) + seconds(rusage.ru_stime),
		});
	}
	Ok(processes)
}

/// Reads the process name, which is a NUL terminated `char[COMMLEN + 1]`.
fn name(entry: &[u8], offset: usize) -> String {
	let bytes = &entry[offset..offset + libc::COMMLEN + 1];
	let length = bytes
		.iter()
		.position(|&byte| byte == 0)
		.unwrap_or(bytes.len());
	String::from_utf8_lossy(&bytes[..length]).into_owned()
}

fn seconds(time: libc::timeval) -> f64 {
	time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0
}

/// Reads a value at the given offset of an entry. The caller makes sure the value is within the
/// entry, which the kernel does not necessarily align for us.
fn read<T: Copy>(entry: &[u8], offset: usize) -> T {
	// SAFETY: the caller guarantees that `offset + size_of::<T>()` fits into `entry`, and
	// `read_unaligned` accepts any alignment.
	unsafe { std::ptr::read_unaligned(entry.as_ptr().add(offset) as *const T) }
}

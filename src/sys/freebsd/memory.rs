//! Memory statistics from the `vm.stats` and `vm.swap_*` sysctl nodes.

use std::convert::TryInto;

use super::sysctl;
use super::Result;

/// Physical memory usage, with the same formula as psutil: `used` is everything but the free,
/// inactive and cached pages.
pub struct VirtualMemory {
	total: u64,
	used: u64,
	percent: f32,
}

impl VirtualMemory {
	pub fn total(&self) -> u64 {
		self.total
	}

	pub fn used(&self) -> u64 {
		self.used
	}

	pub fn percent(&self) -> f32 {
		self.percent
	}
}

pub fn virtual_memory() -> Result<VirtualMemory> {
	let page_size = sysctl::number("hw.pagesize")?;
	let total = sysctl::number("vm.stats.vm.v_page_count")? * page_size;
	let free = sysctl::number_or("vm.stats.vm.v_free_count", 0);
	let inactive = sysctl::number_or("vm.stats.vm.v_inactive_count", 0);
	let cache = sysctl::number_or("vm.stats.vm.v_cache_count", 0);
	let available = (free + inactive + cache) * page_size;
	let used = total.saturating_sub(available);

	Ok(VirtualMemory {
		total,
		used,
		percent: percent(used, total),
	})
}

/// Swap usage. `vm.swap_total` holds the total swap in bytes and each entry of `vm.swap_info`
/// reports the number of swap pages in use on that device.
pub struct SwapMemory {
	total: u64,
	used: u64,
	percent: f32,
}

impl SwapMemory {
	pub fn total(&self) -> u64 {
		self.total
	}

	pub fn used(&self) -> u64 {
		self.used
	}

	pub fn percent(&self) -> f32 {
		self.percent
	}
}

pub fn swap_memory() -> Result<SwapMemory> {
	let page_size = sysctl::number("hw.pagesize")?;
	let total = sysctl::number("vm.swap_total")?;
	let devices = sysctl::number_or("vm.nswapdev", 0);

	let mut used: u64 = 0;
	for device in 0..devices {
		let Ok(buffer) = sysctl::bytes(&format!("vm.swap_info.{}", device)) else {
			// A swap device can disappear between reading the device count and the device.
			continue;
		};
		// `struct xswdev` ends in `xsw_used`, which moved from offset 16 to 24 when `dev_t`
		// grew to 64 bits. The kernel reports the size of its copy of the struct, so both
		// layouts can be told apart.
		let offset = match buffer.len() {
			length if length >= 28 => 24,
			20 => 16,
			_ => continue,
		};
		let Some(pages) = buffer.get(offset..offset + 4) else {
			continue;
		};
		let pages = i32::from_ne_bytes(pages.try_into().unwrap());
		if pages > 0 {
			used = used.saturating_add(pages as u64 * page_size);
		}
	}
	let used = used.min(total);

	Ok(SwapMemory {
		total,
		used,
		percent: percent(used, total),
	})
}

fn percent(used: u64, total: u64) -> f32 {
	if total == 0 {
		0.0
	} else {
		(used as f64 / total as f64 * 100.0) as f32
	}
}

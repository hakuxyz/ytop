//! Network io counters from `getifaddrs(3)`.

use std::collections::HashMap;
use std::io;
use std::os::raw::c_char;

use super::Result;

/// Bytes received and sent, summed over all interfaces of interest.
#[derive(Clone, Copy, Debug, Default)]
pub struct NetIoCounters {
	bytes_recv: u64,
	bytes_sent: u64,
}

impl NetIoCounters {
	pub fn bytes_recv(&self) -> u64 {
		self.bytes_recv
	}

	pub fn bytes_sent(&self) -> u64 {
		self.bytes_sent
	}
}

impl std::iter::Sum for NetIoCounters {
	fn sum<I: Iterator<Item = NetIoCounters>>(iter: I) -> NetIoCounters {
		iter.fold(NetIoCounters::default(), |mut total, counters| {
			total.bytes_recv = total.bytes_recv.saturating_add(counters.bytes_recv);
			total.bytes_sent = total.bytes_sent.saturating_add(counters.bytes_sent);
			total
		})
	}
}

#[derive(Default)]
pub struct NetIoCountersCollector {}

impl NetIoCountersCollector {
	/// The io counters of every interface, taken from the link layer entries where
	/// `struct if_data` is attached.
	pub fn net_io_counters_pernic(&mut self) -> Result<Vec<(String, NetIoCounters)>> {
		let mut counters: HashMap<String, NetIoCounters> = HashMap::new();
		let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
		// SAFETY: `list` is a valid out pointer, the returned linked list only has to be
		// released with `freeifaddrs` once, which happens after the loop on every path.
		if unsafe { libc::getifaddrs(&mut list) } != 0 {
			return Err(io::Error::last_os_error().into());
		}

		let mut entry = list;
		while !entry.is_null() {
			// SAFETY: `entry` points at one node of the list allocated by `getifaddrs`.
			let interface = unsafe { &*entry };
			let is_link_layer = !interface.ifa_addr.is_null()
				&& !interface.ifa_data.is_null()
				// SAFETY: `ifa_addr` points at a valid socket address.
				&& unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_LINK;
			if is_link_layer {
				if let Some(name) = string(interface.ifa_name) {
					// SAFETY: link layer entries carry a `struct if_data` in `ifa_data`.
					let data = unsafe { &*(interface.ifa_data as *const libc::if_data) };
					let counter = counters.entry(name).or_default();
					counter.bytes_recv = counter.bytes_recv.saturating_add(data.ifi_ibytes);
					counter.bytes_sent = counter.bytes_sent.saturating_add(data.ifi_obytes);
				}
			}
			entry = interface.ifa_next;
		}
		unsafe { libc::freeifaddrs(list) };

		Ok(counters.into_iter().collect())
	}
}

fn string(pointer: *const c_char) -> Option<String> {
	if pointer.is_null() {
		return None;
	}
	// SAFETY: interface names are NUL terminated strings.
	let bytes = unsafe { std::ffi::CStr::from_ptr(pointer) }.to_bytes();
	Some(String::from_utf8_lossy(bytes).into_owned())
}

//! Host information from `gethostname(3)`.

use std::os::raw::c_char;

pub struct HostInfo {
	hostname: String,
}

impl HostInfo {
	pub fn hostname(&self) -> &str {
		&self.hostname
	}
}

pub fn info() -> HostInfo {
	HostInfo {
		hostname: hostname(),
	}
}

fn hostname() -> String {
	let mut buffer = vec![0_u8; 256];
	// SAFETY: `buffer` holds 256 writable bytes and the length matches, so `gethostname` can
	// only fail without writing outside of the buffer.
	let status = unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut c_char, buffer.len()) };
	if status != 0 {
		return "unknown".to_string();
	}
	let end = buffer
		.iter()
		.position(|&byte| byte == 0)
		.unwrap_or(buffer.len());
	String::from_utf8_lossy(&buffer[..end]).into_owned()
}

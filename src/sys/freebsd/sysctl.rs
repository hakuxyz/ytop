//! Helpers around `sysctlbyname(3)`.

use std::convert::TryInto;
use std::ffi::CString;
use std::io;
use std::os::raw::{c_char, c_void};

/// Reads a sysctl node into a byte buffer.
///
/// The size of the value is queried first, so the buffer is big enough for the read. If the value
/// grows between the two calls, the read is retried with a bigger buffer.
pub fn bytes(name: &str) -> io::Result<Vec<u8>> {
	for _ in 0..3 {
		let mut length = 0;
		sysctl(name, std::ptr::null_mut(), &mut length)?;
		if length == 0 {
			return Ok(Vec::new());
		}
		let mut buffer = vec![0; length];
		match sysctl(name, buffer.as_mut_ptr() as *mut c_void, &mut length) {
			Ok(()) => {
				buffer.truncate(length);
				return Ok(buffer);
			}
			// The value grew between the size query and the read, so start over.
			Err(error) if error.raw_os_error() == Some(libc::ENOMEM) => continue,
			Err(error) => return Err(error),
		}
	}
	Err(io::Error::other(format!(
		"the sysctl node {} kept growing while reading it",
		name
	)))
}

/// Reads a sysctl node that holds a single unsigned integer of up to 8 bytes.
pub fn number(name: &str) -> io::Result<u64> {
	let buffer = bytes(name)?;
	match buffer.len() {
		1 => Ok(buffer[0] as u64),
		2 => Ok(u16::from_ne_bytes([buffer[0], buffer[1]]) as u64),
		4 => Ok(u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as u64),
		8 => Ok(u64::from_ne_bytes(buffer[..8].try_into().unwrap())),
		length => Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("unexpected size {} for the sysctl node {}", length, name),
		)),
	}
}

/// Reads a sysctl node that holds a single signed integer of up to 8 bytes.
pub fn signed(name: &str) -> io::Result<i64> {
	let buffer = bytes(name)?;
	match buffer.len() {
		1 => Ok(buffer[0] as i8 as i64),
		2 => Ok(i16::from_ne_bytes([buffer[0], buffer[1]]) as i64),
		4 => Ok(i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as i64),
		8 => Ok(i64::from_ne_bytes(buffer[..8].try_into().unwrap())),
		length => Err(io::Error::new(
			io::ErrorKind::InvalidData,
			format!("unexpected size {} for the sysctl node {}", length, name),
		)),
	}
}

/// Reads an unsigned integer sysctl node, or the given default if it does not exist.
pub fn number_or(name: &str, default: u64) -> u64 {
	number(name).unwrap_or(default)
}

fn sysctl(name: &str, old_value: *mut c_void, old_length: *mut usize) -> io::Result<()> {
	let name = CString::new(name).map_err(|_| {
		io::Error::new(
			io::ErrorKind::InvalidInput,
			"sysctl node names contain a NUL byte",
		)
	})?;
	// SAFETY: `name` is a valid NUL terminated string, `old_value` either is NULL or points at
	// `*old_length` writable bytes, and FreeBSD writes the size of the value through
	// `old_length`. No value is written, so `new_value` and `new_length` are unused.
	let status =
		unsafe { libc::sysctlbyname(name.as_ptr(), old_value, old_length, std::ptr::null(), 0) };
	if status != 0 {
		return Err(io::Error::last_os_error());
	}
	Ok(())
}

/// Converts a fixed size NUL terminated `c_char` array into a string.
pub fn fixed_string(bytes: &[c_char]) -> String {
	let characters: Vec<u8> = bytes
		.iter()
		.take_while(|&&byte| byte != 0)
		.map(|&byte| byte as u8)
		.collect();
	String::from_utf8_lossy(&characters).into_owned()
}

//! Platform specific helpers.

/// Reads the commandline of the process with the given pid on FreeBSD.
///
/// FreeBSD has no procfs equivalent of the argument vector, so the `kern.proc.args.<pid>` sysctl
/// is used instead. Returns `None` if the process exited, if there are insufficient permissions
/// or if the process was started without arguments.
#[cfg(target_os = "freebsd")]
pub fn process_commandline(pid: u32) -> Option<String> {
	let bytes = crate::sys::freebsd::sysctl::bytes(&format!("kern.proc.args.{}", pid)).ok()?;
	// The sysctl returns the argument vector as a sequence of NUL terminated strings.
	let arguments = bytes
		.split(|&byte| byte == 0)
		.filter(|argument| !argument.is_empty())
		.map(|argument| String::from_utf8_lossy(argument).into_owned())
		.collect::<Vec<_>>();
	if arguments.is_empty() {
		return None;
	}
	Some(arguments.join(" "))
}

/// Reads the commandline of the process with the given pid on macOS.
///
/// macOS has no procfs equivalent of the argument vector, so the `KERN_PROCARGS2` sysctl is used
/// instead. Returns `None` if the process exited, if there are insufficient permissions or if
/// the buffer could not be read.
#[cfg(target_os = "macos")]
pub fn process_commandline(pid: u32) -> Option<String> {
	let mut mib: [libc::c_int; 3] = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
	let mut size: usize = 0;

	// The first call determines the size of the buffer that is needed.
	unsafe {
		if libc::sysctl(
			mib.as_mut_ptr(),
			mib.len() as libc::c_uint,
			std::ptr::null_mut(),
			&mut size,
			std::ptr::null_mut(),
			0,
		) != 0 || size == 0
		{
			return None;
		}
	}

	// The argument vector can grow between the two calls, in which case the sysctl fails and
	// reports the size that is actually needed through `len`, so retry with a bigger buffer.
	for _ in 0..3 {
		let mut buf = vec![0_u8; size];
		let mut len = size;
		let status = unsafe {
			libc::sysctl(
				mib.as_mut_ptr(),
				mib.len() as libc::c_uint,
				buf.as_mut_ptr() as *mut libc::c_void,
				&mut len,
				std::ptr::null_mut(),
				0,
			)
		};
		if status != 0 {
			if len > size {
				size = len;
				continue;
			}
			return None;
		}
		buf.truncate(len);
		return parse_cmdline(&buf);
	}

	None
}

/// Parses the buffer returned by the `KERN_PROCARGS2` sysctl into a commandline.
///
/// The buffer is laid out as the argument count as a native endian `i32`, followed by the
/// executable path, a `NUL`, the arguments each `NUL` terminated, and finally the environment of
/// the process.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_cmdline(buf: &[u8]) -> Option<String> {
	if buf.len() < 4 {
		return None;
	}

	let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
	if argc <= 0 {
		return None;
	}
	let argc = argc as usize;

	// Skip over the executable path and the `NUL`s separating it from the arguments.
	let rest = &buf[4..];
	let mut index = rest.iter().position(|&byte| byte == 0)? + 1;
	while index < rest.len() && rest[index] == 0 {
		index += 1;
	}

	let mut args: Vec<String> = Vec::with_capacity(argc);
	while args.len() < argc {
		match rest
			.get(index..)
			.and_then(|rest| rest.iter().position(|&byte| byte == 0))
		{
			Some(offset) => {
				args.push(String::from_utf8_lossy(&rest[index..index + offset]).into_owned());
				index += offset + 1;
			}
			None => break,
		}
	}

	if args.is_empty() {
		return None;
	}
	Some(args.join(" "))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cmdline_buf(argc: i32, path: &str, args: &[&str]) -> Vec<u8> {
		let mut buf = argc.to_ne_bytes().to_vec();
		buf.extend_from_slice(path.as_bytes());
		buf.push(0);
		buf.push(0);
		for arg in args {
			buf.extend_from_slice(arg.as_bytes());
			buf.push(0);
		}
		// The environment follows the arguments and must not show up in the commandline.
		buf.extend_from_slice(b"PATH=/usr/bin\0SHELL=/bin/sh\0");
		buf
	}

	#[test]
	fn parses_arguments() {
		let buf = cmdline_buf(3, "/usr/bin/cargo", &["cargo", "build", "--release"]);
		assert_eq!(
			parse_cmdline(&buf),
			Some("cargo build --release".to_string())
		);
	}

	#[test]
	fn returns_partial_arguments_if_the_buffer_is_truncated() {
		let mut buf = 3_i32.to_ne_bytes().to_vec();
		buf.extend_from_slice(b"/bin/echo\0\0echo\0hello\0");
		assert_eq!(parse_cmdline(&buf), Some("echo hello".to_string()));
	}

	#[test]
	fn parses_single_argument() {
		let buf = cmdline_buf(1, "/sbin/init", &["/sbin/init"]);
		assert_eq!(parse_cmdline(&buf), Some("/sbin/init".to_string()));
	}

	#[test]
	fn parses_argument_with_spaces() {
		let buf = cmdline_buf(2, "/bin/sh", &["sh", "-c echo hello"]);
		assert_eq!(parse_cmdline(&buf), Some("sh -c echo hello".to_string()));
	}

	#[test]
	fn ignores_too_small_buffers() {
		assert_eq!(parse_cmdline(&[]), None);
		assert_eq!(parse_cmdline(&[0, 0, 0]), None);
		assert_eq!(parse_cmdline(&0_i32.to_ne_bytes()), None);
	}

	#[test]
	fn ignores_buffer_without_arguments() {
		let mut buf = 1_i32.to_ne_bytes().to_vec();
		buf.extend_from_slice(b"/bin/true\0");
		assert_eq!(parse_cmdline(&buf), None);
	}
}

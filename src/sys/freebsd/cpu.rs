//! CPU usage from the `kern.cp_time` and `kern.cp_times` sysctl nodes.

use super::sysctl;
use super::Result;

/// The number of logical CPUs.
pub fn cpu_count() -> u64 {
	sysctl::number("kern.ncpu").unwrap_or(1)
}

/// `CP_USER`, `CP_NICE`, `CP_SYS`, `CP_INTR` and `CP_IDLE` in scheduler ticks.
const CPUSTATES: usize = 5;

type CpuTimes = [u64; CPUSTATES];

/// Returns the percentage of time the CPU was not idle since the previous reading.
pub struct CpuPercentCollector {
	previous_total: Option<CpuTimes>,
	previous_percpu: Option<Vec<CpuTimes>>,
}

impl CpuPercentCollector {
	/// Takes an initial reading so that the first call to `cpu_percent` already returns a value
	/// relative to the startup of ytop.
	pub fn new() -> Result<CpuPercentCollector> {
		let previous_total = read_total()?;
		let previous_percpu = read_times("kern.cp_times")?;

		Ok(CpuPercentCollector {
			previous_total: Some(previous_total),
			previous_percpu: Some(previous_percpu),
		})
	}

	/// Returns the percentage of time the CPU was not idle since the last call.
	pub fn cpu_percent(&mut self) -> Result<f32> {
		let times = read_total()?;
		let percent = self
			.previous_total
			.map(|previous| usage(previous, times))
			.unwrap_or(0.0);
		self.previous_total = Some(times);
		Ok(percent)
	}

	/// Returns the percentage of time each CPU was not idle since the last call.
	pub fn cpu_percent_percpu(&mut self) -> Result<Vec<f32>> {
		let times = read_times("kern.cp_times")?;
		let percents = match self.previous_percpu.take() {
			Some(previous) if previous.len() == times.len() => previous
				.iter()
				.zip(times.iter())
				.map(|(previous, times)| usage(*previous, *times))
				.collect(),
			// The number of CPUs changed, so the first reading is a baseline again.
			_ => vec![0.0; times.len()],
		};
		self.previous_percpu = Some(times);
		Ok(percents)
	}
}

/// Reads the aggregate cpu times, which are a single `CPUSTATES` long array.
fn read_total() -> Result<CpuTimes> {
	let times = read_times("kern.cp_time")?;
	if times.len() != 1 {
		return Err("unexpected number of cpu states for the whole system".into());
	}
	Ok(times[0])
}

fn read_times(name: &str) -> Result<Vec<CpuTimes>> {
	let bytes = sysctl::bytes(name)?;
	let (ticks, remainder) = bytes.as_chunks::<8>();
	if ticks.is_empty() || !remainder.is_empty() || ticks.len() % CPUSTATES != 0 {
		return Err(format!("unexpected size of the sysctl node {}", name).into());
	}
	Ok(ticks
		.as_chunks::<CPUSTATES>()
		.0
		.iter()
		.map(|states| {
			let mut times = [0; CPUSTATES];
			for (time, tick) in times.iter_mut().zip(states) {
				// Scheduler ticks are `long` and never overflow into the sign bit in practice.
				*time = i64::from_ne_bytes(*tick) as u64;
			}
			times
		})
		.collect())
}

fn usage(previous: CpuTimes, current: CpuTimes) -> f32 {
	let delta = |state: usize| current[state].saturating_sub(previous[state]);
	// CP_USER + CP_NICE + CP_SYS + CP_INTR
	let busy = (0..CPUSTATES - 1).map(delta).sum::<u64>();
	let idle = delta(CPUSTATES - 1);
	let total = busy + idle;
	if total == 0 {
		0.0
	} else {
		(busy as f64 / total as f64 * 100.0) as f32
	}
}

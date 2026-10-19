//! Temperature sensors from the `dev.cpu.*.temperature` and
//! `hw.acpi.thermal.*.temperature` sysctl nodes.

use super::sysctl;
use super::Result;

/// The number of CPUs and thermal zones that are probed.
const MAX_CPU_INDEX: i64 = 32;
const MAX_THERMAL_ZONE_INDEX: i64 = 8;

pub fn temperatures() -> Vec<Result<Sensor>> {
	let mut sensors = Vec::new();

	for index in 0..MAX_CPU_INDEX {
		if let Ok(raw) = sysctl::signed(&format!("dev.cpu.{}.temperature", index)) {
			if let Some(celsius) = decode(raw) {
				sensors.push(Ok(Sensor {
					unit: "cpu".to_string(),
					label: Some(index.to_string()),
					current: Temperature { celsius },
				}));
			}
		}
	}
	for index in 0..MAX_THERMAL_ZONE_INDEX {
		if let Ok(raw) = sysctl::signed(&format!("hw.acpi.thermal.tz{}.temperature", index)) {
			if let Some(celsius) = decode(raw) {
				sensors.push(Ok(Sensor {
					unit: "acpi".to_string(),
					label: Some(format!("tz{}", index)),
					current: Temperature { celsius },
				}));
			}
		}
	}

	sensors
}

pub struct Sensor {
	unit: String,
	label: Option<String>,
	current: Temperature,
}

impl Sensor {
	pub fn label(&self) -> Option<&str> {
		self.label.as_deref()
	}

	pub fn unit(&self) -> &str {
		&self.unit
	}

	pub fn current(&self) -> Temperature {
		self.current
	}
}

#[derive(Clone, Copy)]
pub struct Temperature {
	celsius: f64,
}

impl Temperature {
	pub fn celsius(&self) -> f64 {
		self.celsius
	}

	pub fn fahrenheit(&self) -> f64 {
		self.celsius * 9.0 / 5.0 + 32.0
	}
}

/// Temperature sysctls either report whole degrees Celsius or, with the `IK` format used by
/// coretemp, amdtemp and the ACPI thermal driver, tenths of a Kelvin. A value of zero or less
/// means that the sensor is unavailable.
fn decode(raw: i64) -> Option<f64> {
	match raw {
		value if value <= 0 => None,
		// Tenths of a Kelvin, so 46.0 degrees Celsius is reported as 3192.
		value if value >= 1000 => Some(value as f64 / 10.0 - 273.15),
		// Tenths of a degree Celsius.
		value if value >= 100 => Some(value as f64 / 10.0),
		value => Some(value as f64),
	}
}

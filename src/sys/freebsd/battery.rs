//! Batteries from the `hw.acpi.battery.*` sysctl nodes.
//!
//! The sysctls report the accumulated status of all batteries, so all of them show up as a
//! single entry, which is what the widgets expect as well.

use super::sysctl;
use super::Result;

pub struct Manager;

impl Manager {
	pub fn new() -> Result<Manager> {
		Ok(Manager)
	}

	/// All batteries, or an empty list on machines without ACPI batteries.
	pub fn batteries(&self) -> Result<Vec<Result<Battery>>> {
		let Ok(units) = sysctl::signed("hw.acpi.battery.units") else {
			// Machines without the acpi_battery driver simply have no batteries.
			return Ok(Vec::new());
		};
		if units <= 0 {
			return Ok(Vec::new());
		}
		// A machine that reports batteries must be able to report their charge.
		let life = sysctl::signed("hw.acpi.battery.life")?;

		Ok(vec![Ok(Battery {
			model: "acpi0".to_string(),
			state_of_charge: life.clamp(0, 100) as f32 / 100.0,
		})])
	}
}

pub struct Battery {
	model: String,
	state_of_charge: f32,
}

impl Battery {
	pub fn model(&self) -> Option<&str> {
		Some(&self.model)
	}

	pub fn state_of_charge(&self) -> Charge {
		Charge {
			value: self.state_of_charge,
		}
	}
}

pub struct Charge {
	pub value: f32,
}

use std::collections::HashMap;
use std::ops::Not;
use std::process::Command;

use crate::sys::cpu;
use crate::sys::memory;
use crate::sys::process;
#[cfg(target_os = "linux")]
use crate::sys::process::os::linux::Oneshot;
use num_rational::Ratio;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Row, Table, Widget};

use crate::colorscheme::Colorscheme;
use crate::update::UpdatableWidget;
use crate::widgets::block;

const UP_ARROW: &str = "▲";
const DOWN_ARROW: &str = "▼";

#[derive(PartialEq, Default)]
enum SortMethod {
	#[default]
	Cpu,
	Mem,
	Num,
	Command,
}

#[derive(PartialEq, Clone, Copy, Default)]
enum SortDirection {
	Up,
	#[default]
	Down,
}

impl Not for SortDirection {
	type Output = SortDirection;

	fn not(self) -> Self::Output {
		match self {
			SortDirection::Up => SortDirection::Down,
			SortDirection::Down => SortDirection::Up,
		}
	}
}

enum SelectedProc {
	Pid(u32),
	Name(String),
}

#[derive(Clone)]
struct Proc {
	num: u32,
	name: String,
	commandline: String,
	cpu: f32,
	mem: f32,
}

/// Case-insensitive substring match against the process name and commandline.
fn matches_filter(proc: &Proc, filter: &str) -> bool {
	let filter = filter.to_lowercase();
	proc.name.to_lowercase().contains(&filter) || proc.commandline.to_lowercase().contains(&filter)
}

pub struct ProcWidget<'a> {
	title: String,
	update_interval: Ratio<u64>,
	colorscheme: &'a Colorscheme,

	grouping: bool,
	selected_row: usize,
	selected_proc: Option<SelectedProc>,
	sort_method: SortMethod,
	sort_direction: SortDirection,
	view_offset: usize,
	scrolled: bool,
	view_height: usize,

	filter: String,
	filter_editing: bool,
	row_count: usize,
	area: Rect,

	cpu_count: u64,

	procs: Vec<Proc>,
	grouped_procs: HashMap<String, Proc>,

	process_collector: process::ProcessCollector,
}

impl ProcWidget<'_> {
	pub fn new(colorscheme: &Colorscheme) -> ProcWidget<'_> {
		ProcWidget {
			title: "Processes".to_string(),
			update_interval: Ratio::from_integer(1),
			colorscheme,

			grouping: true,
			selected_row: 0,
			selected_proc: None,
			sort_method: SortMethod::default(),
			sort_direction: SortDirection::default(),
			view_offset: 0,
			scrolled: false,
			view_height: 0,

			filter: String::new(),
			filter_editing: false,
			row_count: 0,
			area: Rect::default(),

			cpu_count: cpu::cpu_count(),

			procs: Vec::new(),
			grouped_procs: HashMap::new(),

			process_collector: process::ProcessCollector::new().unwrap(),
		}
	}

	fn scroll_count(&mut self, count: isize) {
		self.selected_row = isize::max(0, self.selected_row as isize + count) as usize;
		self.selected_proc = None;
		self.scrolled = true;
	}

	fn scroll_to(&mut self, count: usize) {
		self.selected_row = usize::min(count, self.row_count.saturating_sub(1));
		self.selected_proc = None;
		self.scrolled = true;
	}

	pub fn scroll_up(&mut self) {
		self.scroll_count(-1);
	}

	pub fn scroll_down(&mut self) {
		self.scroll_count(1);
	}

	pub fn scroll_top(&mut self) {
		self.scroll_to(0);
	}

	pub fn scroll_bottom(&mut self) {
		self.scroll_to(usize::MAX);
	}

	pub fn scroll_half_page_down(&mut self) {
		self.scroll_count(self.view_height as isize / 2);
	}

	pub fn scroll_half_page_up(&mut self) {
		self.scroll_count(-(self.view_height as isize / 2));
	}

	pub fn scroll_full_page_down(&mut self) {
		self.scroll_count(self.view_height as isize);
	}

	pub fn scroll_full_page_up(&mut self) {
		self.scroll_count(-(self.view_height as isize));
	}

	pub fn toggle_grouping(&mut self) {
		self.grouping = !self.grouping;
		self.selected_proc = None;
	}

	pub fn is_filter_editing(&self) -> bool {
		self.filter_editing
	}

	pub fn has_filter(&self) -> bool {
		!self.filter.is_empty()
	}

	pub fn start_filter_editing(&mut self) {
		self.filter_editing = true;
	}

	pub fn stop_filter_editing(&mut self) {
		self.filter_editing = false;
	}

	pub fn filter_push(&mut self, c: char) {
		self.filter.push(c);
	}

	pub fn filter_pop(&mut self) {
		self.filter.pop();
	}

	pub fn clear_filter(&mut self) {
		self.filter.clear();
		self.filter_editing = false;
	}

	/// Selects the process under the given terminal cell, if the cell is inside the widget and on
	/// top of a process row.
	pub fn click(&mut self, x: u16, y: u16) {
		if x < self.area.x
			|| x >= self.area.x + self.area.width
			|| y < self.area.y
			|| y >= self.area.y + self.area.height
		{
			return;
		}
		// The first row below the top border and the header row.
		let first_row_y = self.area.y + 2;
		if y < first_row_y {
			return;
		}
		let selected = self.view_offset + (y - first_row_y) as usize;
		if selected >= self.row_count {
			return;
		}
		self.selected_row = selected;
		self.selected_proc = None;
		self.scrolled = true;
	}

	pub fn kill_process(&self) {
		let (command, arg) = match self.selected_proc.as_ref() {
			Some(SelectedProc::Pid(pid)) => ("kill", pid.to_string()),
			Some(SelectedProc::Name(name)) => ("pkill", name.clone()),
			None => return,
		};
		// The process may have exited by the time we send the signal, so a failure to spawn the
		// signal command isn't fatal.
		let _ = Command::new(command).arg(arg).spawn();
	}

	fn sort(&mut self, sort_method: SortMethod) {
		if self.sort_method == sort_method {
			self.sort_direction = !self.sort_direction;
		} else {
			self.sort_method = sort_method;
			self.sort_direction = SortDirection::default();
		}
	}

	pub fn sort_by_num(&mut self) {
		self.sort(SortMethod::Num);
	}

	pub fn sort_by_command(&mut self) {
		self.sort(SortMethod::Command);
	}

	pub fn sort_by_cpu(&mut self) {
		self.sort(SortMethod::Cpu);
	}

	pub fn sort_by_mem(&mut self) {
		self.sort(SortMethod::Mem);
	}
}

impl UpdatableWidget for ProcWidget<'_> {
	fn update(&mut self) {
		self.process_collector.update().unwrap();

		let cpu_count = self.cpu_count as f32;
		let virtual_memory = memory::virtual_memory().unwrap();

		self.procs = self
			.process_collector
			.processes
			.values_mut()
			.map(|process| {
				let num = process.pid();

				#[cfg(target_os = "linux")]
				let name = process.name_oneshot();
				#[cfg(target_os = "macos")]
				let name = process.name()?;
				#[cfg(target_os = "freebsd")]
				let name = process.name();

				#[cfg(target_os = "linux")]
				let commandline = process.cmdline()?.unwrap_or_else(|| format!("[{}]", name));
				#[cfg(target_os = "macos")]
				let commandline = crate::platform::process_commandline(num)
					.filter(|commandline| !commandline.is_empty())
					.unwrap_or_else(|| format!("[{}]", name));
				#[cfg(target_os = "freebsd")]
				let commandline = crate::platform::process_commandline(num)
					.filter(|commandline| !commandline.is_empty())
					.unwrap_or_else(|| format!("[{}]", name));

				#[cfg(target_os = "linux")]
				let cpu = process.cpu_percent_oneshot() / cpu_count;
				#[cfg(target_os = "macos")]
				let cpu = process.cpu_percent()? / cpu_count;
				#[cfg(target_os = "freebsd")]
				let cpu = process.cpu_percent() / cpu_count;

				let mem = process.memory_percent_oneshot(&virtual_memory)?;

				Ok(Proc {
					num,
					name,
					commandline,
					cpu,
					mem,
				})
			})
			.filter_map(|process: process::ProcessResult<Proc>| process.ok())
			.collect();

		self.grouped_procs.clear();
		for proc in self.procs.iter() {
			self.grouped_procs
				.entry(proc.name.clone())
				.and_modify(|e| {
					e.num += 1;
					e.cpu += proc.cpu;
					e.mem += proc.mem;
				})
				.or_insert_with(|| Proc {
					num: 1,
					..proc.clone()
				});
		}
	}

	fn get_update_interval(&self) -> Ratio<u64> {
		self.update_interval
	}
}

impl Widget for &mut ProcWidget<'_> {
	fn render(self, area: Rect, buf: &mut Buffer) {
		if area.height < 3 {
			return;
		}

		self.area = area;

		let inner = Rect {
			x: area.x + 1,
			y: area.y + 1,
			width: area.width - 2,
			height: area.height - 2,
		};

		self.view_height = inner.height as usize - 1;

		let mut procs = if self.grouping {
			self.grouped_procs.values().cloned().collect()
		} else {
			self.procs.clone()
		};
		if !self.filter.is_empty() {
			procs.retain(|proc| matches_filter(proc, &self.filter));
		}
		if self.sort_direction == SortDirection::Up {
			match self.sort_method {
				SortMethod::Cpu => procs.sort_by(|a, b| a.cpu.partial_cmp(&b.cpu).unwrap()),
				SortMethod::Mem => procs.sort_by(|a, b| a.mem.partial_cmp(&b.mem).unwrap()),
				SortMethod::Num => procs.sort_by_key(|a| a.num),
				SortMethod::Command => procs.sort_by(|a, b| a.commandline.cmp(&b.commandline)),
			}
		} else {
			match self.sort_method {
				SortMethod::Cpu => procs.sort_by(|b, a| a.cpu.partial_cmp(&b.cpu).unwrap()),
				SortMethod::Mem => procs.sort_by(|b, a| a.mem.partial_cmp(&b.mem).unwrap()),
				SortMethod::Num => procs.sort_by_key(|proc| std::cmp::Reverse(proc.num)),
				SortMethod::Command => procs.sort_by(|b, a| a.commandline.cmp(&b.commandline)),
			}
		}

		let mut header = [
			if self.grouping { " Count" } else { " PID" },
			"Command",
			"CPU%",
			"Mem%",
		];
		let header_index = match &self.sort_method {
			SortMethod::Cpu => 2,
			SortMethod::Mem => 3,
			SortMethod::Num => 0,
			SortMethod::Command => 1,
		};
		let arrow = match &self.sort_direction {
			SortDirection::Up => UP_ARROW,
			SortDirection::Down => DOWN_ARROW,
		};
		let updated_header = format!("{}{}", header[header_index], arrow);
		header[header_index] = &updated_header;

		self.row_count = procs.len();
		if self.row_count == 0 {
			self.selected_row = 0;
			self.view_offset = 0;
			self.selected_proc = None;
		} else {
			self.selected_row = match &self.selected_proc {
				Some(selected_proc) => {
					match selected_proc {
						SelectedProc::Pid(pid) => procs.iter().position(|proc| proc.num == *pid),
						SelectedProc::Name(name) => {
							procs.iter().position(|proc| proc.name == *name)
						}
					}
				}
				.unwrap_or(self.selected_row),
				None => self.selected_row,
			};
			self.scroll_to(self.selected_row);
			self.selected_proc = if self.grouping {
				Some(SelectedProc::Name(
					procs[self.selected_row].name.to_string(),
				))
			} else {
				Some(SelectedProc::Pid(procs[self.selected_row].num))
			};

			if self.scrolled {
				self.scrolled = false;
				if self.selected_row + 2 > self.view_offset + inner.height as usize {
					self.view_offset = self.selected_row + 2 - inner.height as usize;
				}
			}
			// The number of rows can shrink underneath the viewport, for instance when a filter
			// is applied, which would otherwise leave the selection out of view.
			if self.selected_row < self.view_offset {
				self.view_offset = self.selected_row;
			}
		}

		let filter_suffix = if self.filter_editing {
			format!(" [filter: {}_]", self.filter)
		} else if !self.filter.is_empty() {
			format!(" [filter: {}]", self.filter)
		} else {
			String::new()
		};
		let procs_count = procs.len();
		let view_start = if procs_count == 0 {
			0
		} else {
			self.view_offset + 1
		};
		let view_end = usize::min(self.view_offset + self.view_height, procs_count);
		Table::new(
			procs.into_iter().skip(self.view_offset).map(|proc| {
				Row::new(vec![
					format!(" {}", proc.num),
					if self.grouping {
						proc.name
					} else {
						proc.commandline
					},
					format!("{:>5.1}", proc.cpu),
					format!("{:>4.1}", proc.mem),
				])
				.style(self.colorscheme.text)
			}),
			[
				// max PID can be 4194304 (7 digits) + 1 for padding
				Constraint::Length(8),
				// Constraint::Min(5),
				// width - (left + right border) - (column1 + column3 + column4 width) - (spaces
				// between colums)
				Constraint::Length(u16::max((area.width as i16 - 2 - 18 - 3) as u16, 5)),
				Constraint::Length(5),
				Constraint::Length(5),
			],
		)
		.header(Row::new(header).style(self.colorscheme.text.add_modifier(Modifier::BOLD)))
		.block(block::new(
			self.colorscheme,
			&format!(
				" {}{} ({}-{} of {}) ",
				self.title, filter_suffix, view_start, view_end, procs_count
			),
		))
		// TODO: this is only a temporary workaround until we fix the table column resizing
		// https://github.com/cjbassi/ytop/issues/23
		.column_spacing(1)
		.render(area, buf);

		// Draw cursor.
		let cursor_y = inner.y + 1 + self.selected_row as u16 - self.view_offset as u16;
		if self.row_count > 0 && cursor_y < inner.bottom() {
			for i in inner.x..inner.right() {
				let cell = &mut buf[(i, cursor_y)];
				if cell.symbol() != " " {
					cell.set_fg(self.colorscheme.proc_cursor)
						.set_style(Style::default().add_modifier(Modifier::REVERSED));
				} else {
					cell.set_bg(self.colorscheme.proc_cursor);
				}
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn proc(name: &str, commandline: &str) -> Proc {
		Proc {
			num: 42,
			name: name.to_string(),
			commandline: commandline.to_string(),
			cpu: 1.5,
			mem: 2.5,
		}
	}

	#[test]
	fn empty_filter_matches_every_process() {
		let proc = proc("ytop", "ytop");
		assert!(matches_filter(&proc, ""));
	}

	#[test]
	fn filter_matches_process_name() {
		let proc = proc("ytop", "/usr/local/bin/ytop");
		assert!(matches_filter(&proc, "ytop"));
		assert!(matches_filter(&proc, "TOP"));
		assert!(!matches_filter(&proc, "htop"));
	}

	#[test]
	fn filter_matches_commandline() {
		let proc = proc("sh", "sh -c cargo build --release");
		assert!(matches_filter(&proc, "cargo"));
		assert!(matches_filter(&proc, "--RELEASE"));
		assert!(!matches_filter(&proc, "firefox"));
	}
}

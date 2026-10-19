use std::cmp;

use once_cell::sync::Lazy;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::colorscheme::Colorscheme;
use crate::widgets::block;

const TEXT: &str = r"Quit: q or <C-c>
Pause: <Space>
Process navigation:
  - k and <Up>: up
  - j and <Down>: down
  - <C-u>: half page up
  - <C-d>: half page down
  - <C-b>: full page up
  - <C-f>: full page down
  - gg and <Home>: jump to top
  - G and <End>: jump to bottom
Process actions:
  - <Tab>: toggle process grouping
  - dd: kill selected process or process group
Process sorting:
  - p: PID/Count
  - n: Command
  - c: CPU
  - m: Mem
Process filtering:
  - /: start editing filter
  - (while editing):
    - <Enter>: accept filter
  - <C-c> and <Escape>: clear filter
Mouse:
  - click: select process
  - scroll wheel: scroll through processes
CPU and Mem graph scaling:
  - h: scale in
  - l: scale out
Other:
  - ?: toggle this menu";

const TEXT_WIDTH: u16 = 48;
const TEXT_HEIGHT: u16 = 34;

static TEXT_VEC: Lazy<Vec<Line<'static>>> = Lazy::new(|| TEXT.lines().map(Line::raw).collect());

pub struct HelpMenu<'a> {
	title: String,
	colorscheme: &'a Colorscheme,
}

impl HelpMenu<'_> {
	pub fn new(colorscheme: &Colorscheme) -> HelpMenu<'_> {
		HelpMenu {
			title: " Help Menu ".to_string(),
			colorscheme,
		}
	}

	pub fn get_rect(&self, area: Rect) -> Rect {
		Rect {
			x: area.width.saturating_sub(TEXT_WIDTH) / 2,
			y: area.height.saturating_sub(TEXT_HEIGHT) / 2,
			width: cmp::min(TEXT_WIDTH, area.width),
			height: cmp::min(TEXT_HEIGHT, area.height),
		}
	}
}

impl Widget for &HelpMenu<'_> {
	fn render(self, area: Rect, buf: &mut Buffer) {
		Paragraph::new(TEXT_VEC.clone())
			.block(block::new(self.colorscheme, &self.title))
			.render(area, buf);
	}
}

"""Record ytop running in a pseudo terminal and render the output as a GIF.

Usage:
    python3 ./ci/record.py [output.gif] [extra ytop args...]
    python3 ./ci/record.py --still [output.png] [extra ytop args...]

The GIF mode records 88x28 cells, roughly the size of the old demo, for ~13 s
and shows the help menu at the end. A frame is written whenever the screen
changes; the frame duration is the real time until the next change. The --still
mode records 101x23 cells for a few seconds and writes the last frame as a PNG
(the README screenshot). Both modes need target/debug/ytop to exist.
"""
import codecs
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time

from PIL import Image, ImageDraw, ImageFont

COLS, ROWS = 88, 28
FONT_SIZE = 16
FONT_REGULAR = "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Regular.ttf"
FONT_BOLD = "/usr/share/fonts/TTF/JetBrainsMonoNerdFontMono-Bold.ttf"

DEFAULT_FG = (200, 200, 200)
DEFAULT_BG = (0, 0, 0)

ANSI16 = [
	(0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
	(36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
	(102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
	(59, 142, 234), (214, 112, 214), (41, 184, 219), (229, 229, 229),
]


def xterm256(n):
	if n < 16:
		return ANSI16[n]
	if n < 232:
		n -= 16
		levels = [0, 95, 135, 175, 215, 255]
		return (levels[n // 36 % 6], levels[n // 6 % 6], levels[n % 6])
	g = 8 + (n - 232) * 10
	return (g, g, g)


CSI_RE = re.compile(r"\x1b\[([0-9;?<>=]*)([@-~])")
# A CSI sequence split across two reads: everything from the last ESC to the
# end of the buffer can only be a partial sequence and must be held back.
CSI_PARTIAL_RE = re.compile(r"\x1b\[[0-9;?<>=]*$")


class Screen:
	"""Terminal emulator with just enough of VT100 for the sequences ratatui writes."""

	decoder = codecs.getincrementaldecoder("utf-8")("replace")

	def __init__(self):
		self.bg = [[None] * COLS for _ in range(ROWS)]  # None | int 0..255 | (r, g, b)
		self.fg = [[None] * COLS for _ in range(ROWS)]
		self.char = [[" "] * COLS for _ in range(ROWS)]
		self.bold = [[False] * COLS for _ in range(ROWS)]
		self.reverse = [[False] * COLS for _ in range(ROWS)]
		self.row = 0
		self.col = 0
		self.cur_fg = None
		self.cur_bg = None
		self.cur_bold = False
		self.cur_reverse = False
		self.pending = ""

	def _sgr(self, params):
		parts = [int(p) if p else 0 for p in params.split(";")] or [0]
		i = 0
		while i < len(parts):
			n = parts[i]
			if n == 0:
				self.cur_fg = self.cur_bg = None
				self.cur_bold = self.cur_reverse = False
			elif n == 1:
				self.cur_bold = True
			elif n == 7:
				self.cur_reverse = True
			elif n == 22:
				self.cur_bold = False
			elif n == 27:
				self.cur_reverse = False
			elif 30 <= n <= 37:
				self.cur_fg = n - 30
			elif 90 <= n <= 97:
				self.cur_fg = n - 90 + 8
			elif n == 39:
				self.cur_fg = None
			elif 40 <= n <= 47:
				self.cur_bg = n - 40
			elif 100 <= n <= 107:
				self.cur_bg = n - 100 + 8
			elif n == 49:
				self.cur_bg = None
			elif n in (38, 48):
				if i + 2 < len(parts) and parts[i + 1] == 5:
					color = parts[i + 2]
					if n == 38:
						self.cur_fg = color
					else:
						self.cur_bg = color
					i += 2
				elif i + 4 < len(parts) and parts[i + 1] == 2:
					color = (parts[i + 2], parts[i + 3], parts[i + 4])
					if n == 38:
						self.cur_fg = color
					else:
						self.cur_bg = color
					i += 4
			i += 1

	def _put(self, char):
		if self.row < ROWS and self.col < COLS:
			self.char[self.row][self.col] = char
			self.fg[self.row][self.col] = self.cur_fg
			self.bg[self.row][self.col] = self.cur_bg
			self.bold[self.row][self.col] = self.cur_bold
			self.reverse[self.row][self.col] = self.cur_reverse
		self.col += 1
		if self.col >= COLS:
			self.col = 0
			self.row = min(self.row + 1, ROWS - 1)

	def _erase_display(self, mode):
		def clear(r, c0, c1):
			for c in range(c0, c1):
				self.char[r][c] = " "
				self.fg[r][c] = self.bg[r][c] = None
				self.bold[r][c] = self.reverse[r][c] = False

		if mode == 0:
			clear(self.row, self.col, COLS)
			for r in range(self.row + 1, ROWS):
				clear(r, 0, COLS)
		elif mode == 1:
			for r in range(0, self.row):
				clear(r, 0, COLS)
			clear(self.row, 0, self.col + 1)
		else:
			for r in range(ROWS):
				clear(r, 0, COLS)

	def _erase_line(self, mode):
		row = self.char[self.row]
		start, end = (self.col, COLS) if mode == 0 else (0, self.col + 1) if mode == 1 else (0, COLS)
		for c in range(start, end):
			row[c] = " "
			self.fg[self.row][c] = self.bg[self.row][c] = None
			self.bold[self.row][c] = self.reverse[self.row][c] = False

	def feed(self, data):
		text = self.pending + self.decoder.decode(data)
		self.pending = ""
		esc = text.rfind("\x1b")
		if esc >= 0:
			rest = text[esc:]
			if len(rest) == 1 or (rest[1] == "[" and CSI_RE.match(text, esc) is None and CSI_PARTIAL_RE.match(rest)):
				self.pending = rest
				text = text[:esc]
		i = 0
		while i < len(text):
			if text[i] == "\x1b":
				match = CSI_RE.match(text, i)
				if match:
					params, final = match.group(1), match.group(2)
					nums = [int(p) if p else 1 for p in params.split(";")] if params and not params.startswith("?") else []
					if final in "Hf":
						self.row = max(0, min((nums[0] - 1) if nums else 0, ROWS - 1))
						self.col = max(0, min((nums[1] - 1) if len(nums) > 1 else 0, COLS - 1))
					elif final == "J":
						self._erase_display(nums[0] if nums else 0)
					elif final == "K":
						self._erase_line(nums[0] if nums else 0)
					elif final == "m":
						if not params.startswith("?"):
							self._sgr(params)
					i = match.end()
					continue
				i += 1  # unknown escape: skip the ESC
				continue
			char = text[i]
			if char == "\r":
				self.col = 0
			elif char == "\n":
				self.row = min(self.row + 1, ROWS - 1)
			elif char == "\x08":
				self.col = max(0, self.col - 1)
			elif char >= " ":
				self._put(char)
			i += 1

	def snapshot(self):
		return (
			[tuple(r) for r in self.char],
			[tuple(r) for r in self.fg],
			[tuple(r) for r in self.bg],
			[tuple(r) for r in self.bold],
			[tuple(r) for r in self.reverse],
		)

	def content_cells(self):
		return sum(1 for r in range(ROWS) for c in range(COLS) if self.char[r][c] != " ")


def set_winsize(fd, rows, cols):
	fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def render(snap, font, bold_font, cell_w, cell_h):
	char, fg, bg, bold, reverse = snap
	image = Image.new("RGB", (int(cell_w * COLS) + 1, cell_h * ROWS), DEFAULT_BG)
	draw = ImageDraw.Draw(image)
	for r in range(ROWS):
		y = r * cell_h
		for c in range(COLS):
			ch = char[r][c]
			cell_fg, cell_bg = fg[r][c], bg[r][c]
			if reverse[r][c]:
				cell_fg, cell_bg = cell_bg, cell_fg
			fg_color = xterm256(cell_fg) if isinstance(cell_fg, int) else cell_fg or DEFAULT_FG
			bg_color = xterm256(cell_bg) if isinstance(cell_bg, int) else cell_bg or DEFAULT_BG
			if bold[r][c] and isinstance(cell_fg, int) and cell_fg < 8:
				fg_color = xterm256(cell_fg + 8)
			x = int(round(c * cell_w))
			if bg_color != DEFAULT_BG:
				draw.rectangle([x, y, int(round((c + 1) * cell_w)), y + cell_h], fill=bg_color)
			if ch != " ":
				face = bold_font if bold[r][c] else font
				draw.text((x, y - 1), ch, fill=fg_color, font=face)
	return image


def main():
	global COLS, ROWS
	args = [a for a in sys.argv[1:]]
	still = False
	if args and args[0] == "--still":
		still = True
		args.pop(0)
	out = "assets/screenshots/minimal.png" if still else "assets/demos/demo.gif"
	if args and not args[0].startswith("-"):
		out = args.pop(0)
	if still:
		COLS, ROWS = 101, 23
		scripts = [(4.0, None)]
	else:
		# Script: ~9 s of the dashboard, then the help menu for ~3 s, then 1.5 s back.
		scripts = [
			(9.0, b"?"),
			(12.0, b"\x1b"),
			(13.5, None),
		]

	bin_path = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "ytop")
	bin_path = os.path.abspath(bin_path)
	pid, fd = pty.fork()
	if pid == 0:
		os.environ["TERM"] = "xterm-256color"
		# The GIF shows the statusbar, the minimal screenshot the -m layout.
		flags = ["-m"] if still else ["-s"]
		os.execv(bin_path, [bin_path] + flags + args)
	set_winsize(fd, ROWS, COLS)

	font = ImageFont.truetype(FONT_REGULAR, FONT_SIZE)
	bold_font = ImageFont.truetype(FONT_BOLD, FONT_SIZE)
	cell_w = font.getlength("M")
	cell_h = sum(font.getmetrics())

	screen = Screen()
	frames = []  # (time, snapshot)
	start = time.time()
	last_read = start
	dirty = False
	deadline = start + scripts[-1][0]
	step = 0
	last_record = 0.0
	while time.time() < deadline:
		now = time.time() - start
		while step < len(scripts) and now >= scripts[step][0]:
			if scripts[step][1] is not None:
				os.write(fd, scripts[step][1])
			step += 1
		ready, _, _ = select.select([fd], [], [], 0.05)
		if ready:
			try:
				data = os.read(fd, 65536)
			except OSError:
				break
			if not data:
				break
			screen.feed(data)
			last_read = time.time()
			dirty = True
		elif dirty and time.time() - last_read >= 0.15:
			# Snapshot only once the output stream has settled, otherwise big
			# repaints (opening the help menu) are captured halfway through.
			dirty = False
			if time.time() - last_record >= 0.1 and screen.content_cells() > 50:
				frames.append((time.time() - start, screen.snapshot()))
				last_record = time.time()

	os.kill(pid, 9)
	os.waitpid(pid, 0)

	if len(frames) < (2 if still else 5):
		sys.exit("only %d frames recorded" % len(frames))

	# Drop the startup frames where not every widget has been drawn yet.
	while frames and "Processes" not in "".join("".join(row) for row in frames[0][1][0]):
		frames.pop(0)
	if not frames:
		sys.exit("no complete frame recorded")

	images = [render(snap, font, bold_font, cell_w, cell_h) for _, snap in frames]
	if still:
		images[-1].save(out)
		print("wrote %s: %dx%d" % (out, images[-1].width, images[-1].height))
		images[-1].save("/tmp/opencode/frames/frame-still.png")
		return

	durations = []
	for i in range(len(images)):
		delta = frames[i + 1][0] - frames[i][0] if i + 1 < len(frames) else 1.0
		durations.append(max(80, min(1500, int(round(delta * 1000)))))
	images[0].save(
		out,
		save_all=True,
		append_images=images[1:],
		duration=durations,
		loop=0,
		optimize=True,
		disposal=2,
	)
	sizes = os.path.getsize(out)
	print("wrote %s: %d frames, %dx%d, %d bytes" % (out, len(images), images[0].width, images[0].height, sizes))
	print("frame times: %s" % " ".join("%.1fs" % t for t, _ in frames))
	os.makedirs("/tmp/opencode/frames", exist_ok=True)
	for i, image in enumerate(images):
		image.save("/tmp/opencode/frames/frame-%02d.png" % i)


if __name__ == "__main__":
	main()

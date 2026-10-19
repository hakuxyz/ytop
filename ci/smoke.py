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

# Usage: python3 ./ci/smoke.py [path-to-binary]  (run from the repository root)
BIN = sys.argv[1] if len(sys.argv) > 1 else "target/debug/ytop"
ROWS, COLS = 40, 120
CSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
CURSOR_BG = 4  # colorscheme proc_cursor


class Screen:
	"""Minimal terminal emulator: enough to reconstruct the rendered screen."""

	decoder = codecs.getincrementaldecoder("utf-8")("replace")

	def __init__(self):
		self.buf = [[" "] * COLS for _ in range(ROWS)]
		self.attrs = [[(False, None)] * COLS for _ in range(ROWS)]
		self.row = 0
		self.col = 0
		self.reverse = False
		self.bg = None

	def _sgr(self, params):
		nums = [int(p) for p in params.split(";") if p]
		i = 0
		while i < len(nums):
			num = nums[i]
			if num in (38, 48):
				# Color parameters (38;5;N, 48;2;R;G;B) must not be mistaken for
				# attribute parameters such as 7 (reverse).
				if i + 1 < len(nums) and nums[i + 1] == 5:
					if num == 48 and i + 2 < len(nums):
						self.bg = nums[i + 2]
					i += 3
				elif i + 1 < len(nums) and nums[i + 1] == 2:
					if num == 48:
						self.bg = -1
					i += 4
				else:
					if num == 48:
						self.bg = -1
					i += 2
				continue
			if num == 0:
				self.reverse = False
				self.bg = None
			elif num == 7:
				self.reverse = True
			elif num == 27:
				self.reverse = False
			elif num == 49:
				self.bg = None
			i += 1

	def feed(self, data):
		# Decode incrementally so that multibyte characters take a single cell.
		text = self.decoder.decode(data)
		i = 0
		while i < len(text):
			if text[i] == "\x1b":
				match = CSI.match(text, i)
				if match:
					seq = match.group(0)
					if seq.endswith("H"):
						nums = seq[2:-1].split(";")
						row = int(nums[0]) if nums[0] else 1
						col = int(nums[1]) if len(nums) > 1 and nums[1] else 1
						self.row, self.col = row - 1, col - 1
					elif seq.endswith("m"):
						self._sgr(seq[2:-1])
					i = match.end()
					continue
				i += 1
				continue
			char = text[i]
			if char == "\r":
				self.col = 0
			elif char == "\n":
				self.row += 1
			elif char == "\x08":
				self.col = max(0, self.col - 1)
			else:
				if 0 <= self.row < ROWS and 0 <= self.col < COLS:
					self.buf[self.row][self.col] = char
					self.attrs[self.row][self.col] = (self.reverse, self.bg)
				self.col += 1
			i += 1

	def text(self):
		return "\n".join("".join(line) for line in self.buf)

	def cursor_rows(self):
		"""Rows that show the process widget cursor (reversed or highlighted cells)."""
		rows = set()
		for row in range(ROWS):
			for col in range(COLS):
				reverse, bg = self.attrs[row][col]
				if reverse or bg == CURSOR_BG:
					rows.add(row)
					break
		return rows


def set_winsize(fd):
	fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))


def drain(fd, seconds):
	out = b""
	deadline = time.time() + seconds
	while time.time() < deadline:
		ready, _, _ = select.select([fd], [], [], 0.1)
		if ready:
			try:
				chunk = os.read(fd, 65536)
			except OSError:
				break
			if not chunk:
				break
			out += chunk
	return out


def act(screen, fd, data, seconds=0.7):
	drain(fd, 0.3)  # flush pending output so assertions only see the new state
	os.write(fd, data)
	screen.feed(drain(fd, seconds))


failures = []


def check(condition, message):
	if not condition:
		failures.append(message)


pid, fd = pty.fork()
if pid == 0:
	os.environ["TERM"] = "xterm-256color"
	os.execv(BIN, [BIN, "-s"])
set_winsize(fd)

screen = Screen()
raw = b""


def pump(seconds=0.5):
	"""Feed freshly read output into the emulated screen."""
	global raw
	data = drain(fd, seconds)
	raw += data
	screen.feed(data)
	return data


def wait_for(condition, timeout=15.0):
	"""Wait until the condition holds, allowing for slow machines."""
	deadline = time.time() + timeout
	while True:
		if condition():
			return True
		if time.time() >= deadline:
			return condition()
		pump(0.5)


# CI machines can need a while for the first render with a full process list,
# so poll instead of reading for a fixed amount of time.
pump(2.0)
check(b"\x1b[?1000h" in raw or b"\x1b[?1006h" in raw, "mouse capture not enabled")
check(wait_for(lambda: "Processes" in screen.text()), "initial render missing process widget")
check(wait_for(lambda: len(screen.cursor_rows()) == 1), "process cursor not drawn")

# ---------------------------------------------------------------- filter
act(screen, fd, b"/")
check("[filter: _]" in screen.text(), "filter editing prompt not shown")

act(screen, fd, b"ytop")
check("[filter: ytop_]" in screen.text(), "typed filter not shown")

act(screen, fd, b"\r")
text = screen.text()
check("[filter: ytop]" in text, "filter not accepted on enter")
check("[filter: ytop_]" not in text, "editing indicator still shown after enter")
counts = [int(n) for n in re.findall(r"\(1-\d+ of (\d+)\)", text)]
check(
	counts and counts[-1] <= 10,
	"filter did not narrow down the process list (counts: %r)" % counts,
)


act(screen, fd, b"j", 0.5)
check("Processes" in screen.text(), "widget gone after navigation")

act(screen, fd, b"\x1b")
check("[filter" not in screen.text(), "filter not cleared on escape")

# Pause so that data updates can't reorder the list while checking the cursor.
act(screen, fd, b" ", 0.4)

# ---------------------------------------------------------------- mouse
clicked_row = 32
# crossterm expects 1-based SGR coordinates; the emulator works 0-based.
act(screen, fd, b"\x1b[<0;90;%dM" % (clicked_row + 1), 0.8)
check(
	screen.cursor_rows() == {clicked_row},
	"click did not select the process row (cursor rows: %r)" % sorted(screen.cursor_rows()),
)

act(screen, fd, b"\x1b[<65;60;30M", 0.8)  # wheel down
check(
	screen.cursor_rows() == {clicked_row + 1},
	"wheel down did not scroll the selection (cursor rows: %r)" % sorted(screen.cursor_rows()),
)
act(screen, fd, b"\x1b[<64;60;30M", 0.8)  # wheel up
check(
	screen.cursor_rows() == {clicked_row},
	"wheel up did not scroll the selection (cursor rows: %r)" % sorted(screen.cursor_rows()),
)

# ---------------------------------------------------------------- sorting, help, quit
act(screen, fd, b"c")
check("Command" in screen.text(), "process table gone after sorting")
act(screen, fd, b"\t")
act(screen, fd, b"?")
check("Quit: q or <C-c>" in screen.text(), "help menu did not open")
check("Mouse:" in screen.text(), "help menu missing mouse section")
check("scroll wheel" in screen.text(), "help menu missing mouse keybinds")
act(screen, fd, b"\x1b")
check("Processes" in screen.text(), "widget gone after closing help menu")

os.write(fd, b"q")
status = None
deadline = time.time() + 5
while time.time() < deadline:
	wpid, status = os.waitpid(pid, os.WNOHANG)
	if wpid == pid:
		break
	time.sleep(0.1)

if status is None:
	os.kill(pid, 9)
	os.waitpid(pid, 0)
	failures.append("process did not exit on 'q'")
	code = -1
else:
	code = os.waitstatus_to_exitcode(status)

if code != 0:
	failures.append("exit code %s" % code)

if failures:
	print("FAIL")
	for failure in failures:
		print(" -", failure)
	print("--- last screen ---")
	print(screen.text())
	sys.exit(1)

print("OK: filter, mouse select/scroll, sorting, help menu and quit all work")

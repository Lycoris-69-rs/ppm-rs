#!/usr/bin/env python3
"""PTY end-to-end smoke test for the ppm TUI.

Runs the real binary inside a pseudo-terminal, feeds real keystrokes, keeps a
tiny ANSI screen model so assertions can look at *the current screen* (not just
the raw byte stream), and verifies the complete flows:

  main menu -> flat package browser -> metadata -> confirm dialog (cancelled)
  -> Ctrl+D real download -> back to menu -> AUR metadata -> clean exit.

Usage: scripts/pty-smoke.py [--binary target/debug/ppm]
"""

import codecs
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time
import unicodedata

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class Screen:
    """Just enough of a VT100 screen model to read the TUI back as text."""

    def __init__(self, cols, rows):
        self.cols = cols
        self.rows = rows
        self.grid = [[" "] * cols for _ in range(rows)]
        self.cx = 0
        self.cy = 0
        self._decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self._state = "normal"
        self._pending = ""
        self._csi = ""

    def feed(self, data):
        for character in self._decoder.decode(data):
            self._char(character)

    def _char(self, character):
        if self._state == "normal":
            if character == "\x1b":
                self._state = "esc"
            elif character == "\r":
                self.cx = 0
            elif character == "\n":
                self._newline()
            elif character == "\b":
                self.cx = max(0, self.cx - 1)
            elif character == "\t":
                self.cx = min(self.cols - 1, (self.cx // 8 + 1) * 8)
            elif ord(character) < 0x20:
                pass
            else:
                self._put(character)
        elif self._state == "esc":
            if character == "[":
                self._state = "csi"
                self._csi = ""
            elif character == "]":
                self._state = "osc"
            elif character in "()":
                self._state = "charset"
            elif character == "M":
                self.cy = max(0, self.cy - 1)
                self._state = "normal"
            else:
                self._state = "normal"
        elif self._state == "charset":
            self._state = "normal"
        elif self._state == "osc":
            if character == "\x07":
                self._state = "normal"
            elif character == "\x1b":
                self._state = "osc_esc"
        elif self._state == "osc_esc":
            self._state = "normal" if character == "\\" else "osc"
        elif self._state == "csi":
            if 0x40 <= ord(character) <= 0x7E:
                self._apply_csi(self._csi, character)
                self._state = "normal"
            else:
                self._csi += character

    def _newline(self):
        self.cy += 1
        if self.cy >= self.rows:
            self.cy = self.rows - 1
            self.grid.pop(0)
            self.grid.append([" "] * self.cols)

    def _put(self, character):
        width = 2 if unicodedata.east_asian_width(character) in ("W", "F") else 1
        if self.cx >= self.cols:
            self.cx = 0
            self._newline()
        self.grid[self.cy][self.cx] = character
        if width == 2 and self.cx + 1 < self.cols:
            self.grid[self.cy][self.cx + 1] = ""
        self.cx = min(self.cols, self.cx + width)

    @staticmethod
    def _params(raw):
        raw = raw.lstrip("?").lstrip(">").lstrip("!")
        if not raw:
            return []
        values = []
        for part in raw.split(";"):
            try:
                values.append(int(part))
            except ValueError:
                values.append(0)
        return values

    def _apply_csi(self, raw, final):
        params = self._params(raw)
        first = params[0] if params else 0
        if final in ("H", "f"):
            row = (params[0] if len(params) > 0 and params[0] else 1) - 1
            col = (params[1] if len(params) > 1 and params[1] else 1) - 1
            self.cy = max(0, min(self.rows - 1, row))
            self.cx = max(0, min(self.cols - 1, col))
        elif final == "A":
            self.cy = max(0, self.cy - max(1, first))
        elif final == "B":
            self.cy = min(self.rows - 1, self.cy + max(1, first))
        elif final == "C":
            self.cx = min(self.cols - 1, self.cx + max(1, first))
        elif final == "D":
            self.cx = max(0, self.cx - max(1, first))
        elif final == "G":
            self.cx = max(0, min(self.cols - 1, max(1, first) - 1))
        elif final == "d":
            self.cy = max(0, min(self.rows - 1, max(1, first) - 1))
        elif final == "J":
            if first in (2, 3):
                self.grid = [[" "] * self.cols for _ in range(self.rows)]
            elif first == 0:
                self._clear_from_cursor()
        elif final == "K":
            row = self.grid[self.cy]
            if first == 0:
                for x in range(self.cx, self.cols):
                    row[x] = " "
            elif first == 1:
                for x in range(0, min(self.cx + 1, self.cols)):
                    row[x] = " "
            else:
                self.grid[self.cy] = [" "] * self.cols
        elif final == "X":
            for x in range(self.cx, min(self.cols, self.cx + max(1, first))):
                self.grid[self.cy][x] = " "
        elif final in ("h", "l") and raw.startswith("?"):
            if first == 1049:
                self.grid = [[" "] * self.cols for _ in range(self.rows)]
                self.cx = self.cy = 0

    def _clear_from_cursor(self):
        for x in range(self.cx, self.cols):
            self.grid[self.cy][x] = " "
        for y in range(self.cy + 1, self.rows):
            self.grid[y] = [" "] * self.cols

    def text(self):
        return "\n".join("".join(row).rstrip() for row in self.grid)


class Session:
    def __init__(self, binary, env, cols=132, rows=40):
        self.cols, self.rows = cols, rows
        self.screen = Screen(cols, rows)
        self.raw = bytearray()
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            [binary],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            cwd=ROOT,
            env=env,
            close_fds=True,
        )
        os.close(slave)

    def drain(self, duration=0.15):
        deadline = time.time() + duration
        while time.time() < deadline:
            ready, _, _ = select.select([self.master], [], [], 0.05)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                return
            if not chunk:
                return
            self.raw.extend(chunk)
            self.screen.feed(chunk)

    def send(self, data, settle=0.15):
        os.write(self.master, data)
        self.drain(settle)

    def screen_text(self):
        return self.screen.text()

    def wait_for(self, needle, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.drain(0.1)
            if needle in self.screen_text():
                return True
            if self.proc.poll() is not None:
                self.drain(0.2)
                return needle in self.screen_text()
        return False

    def wait_until_gone(self, needle, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.drain(0.1)
            if needle not in self.screen_text():
                return True
        return False

    def save_screen(self, name):
        directory = os.path.join(ROOT, ".dev", "pty-screens")
        os.makedirs(directory, exist_ok=True)
        path = os.path.join(directory, name + ".txt")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(self.screen_text() + "\n")
        return path

    def close(self, timeout=10):
        try:
            os.close(self.master)
        except OSError:
            pass
        try:
            return self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return self.proc.wait(timeout=5)


def main():
    binary = os.path.join(ROOT, "target", "debug", "ppm")
    if len(sys.argv) > 2 and sys.argv[1] == "--binary":
        binary = os.path.abspath(sys.argv[2])
    if not os.path.isfile(binary):
        print(f"FAIL: binary not found: {binary} (run `cargo build` first)")
        return 1

    downloads = os.path.join(ROOT, ".dev", "downloads-test")
    os.makedirs(downloads, exist_ok=True)
    for name in os.listdir(downloads):
        os.remove(os.path.join(downloads, name))
    env = dict(os.environ)
    env.update(
        {
            "TERM": "xterm-256color",
            "PPM_DOWNLOADS_DIR": downloads,
            "PPM_CACHE_DIR": os.path.join(ROOT, ".ppm-cache"),
        }
    )

    failures = []

    # A privileged operation path can be exercised safely when sudo cannot run
    # non-interactively: PPM should surface the failure in its console overlay.
    sudo_available = (
        os.geteuid() == 0
        or subprocess.run(
            ["sudo", "-n", "true"], capture_output=True, check=False
        ).returncode
        == 0
    )

    def check(condition, message):
        print(("ok   " if condition else "FAIL ") + message)
        if not condition:
            failures.append(message)

    session = Session(binary, env)
    try:
        check(session.wait_for("Powerful Package Manager", 30), "main menu renders")
        check(session.wait_for("Install Packages", 5), "menu items render")
        check(
            session.wait_for("packages in one list", 60)
            and "pacman" in session.screen_text()
            and "AUR" in session.screen_text(),
            "package lists load (pacman + AUR)",
        )
        session.save_screen("01-main-menu")

        session.send(b"\r")
        check(session.wait_for("INSTALL PACKAGES", 15), "package browser opens")

        for character in "cowsay":
            session.send(character.encode(), settle=0.05)
        check(session.wait_for("cowsay", 10), "search filters the flat list")
        check(
            session.wait_for("Package Information", 20)
            and "Package Information" in session.screen_text(),
            "info pane shows package metadata",
        )
        session.save_screen("02-install-search")

        session.send(b"\r")
        check(session.wait_for("CONFIRM", 15), "install confirmation dialog appears")
        check(
            "sudo -n pacman -S" in session.screen_text(),
            "dialog shows the exact privileged command",
        )
        session.send(b"n")
        check(session.wait_until_gone("CONFIRM", 5), "confirmation can be cancelled")
        session.save_screen("03-cancelled-confirm")

        session.send(b"\x04")  # Ctrl+D
        check(
            session.wait_for("OPERATION", 20),
            "download operation overlay opens",
        )
        check(
            session.wait_for("Downloading", 5),
            "download overlay shows its title",
        )
        finished = False
        deadline = time.time() + 120
        while time.time() < deadline and not finished:
            session.drain(0.25)
            text = session.screen_text()
            finished = "done —" in text or "saved" in text or "already downloaded" in text
            if session.proc.poll() is not None:
                break
        check(finished, "download completes")
        session.save_screen("04-download-finished")

        files = [
            name
            for name in os.listdir(downloads)
            if name.startswith("cowsay") and not name.endswith(".part")
        ]
        check(bool(files), "package tarball saved to the downloads directory")
        if files:
            size = os.path.getsize(os.path.join(downloads, files[0]))
            check(size > 1024, f"downloaded file looks like a package ({size} bytes)")

        session.send(b"\r")  # close the finished overlay
        check(session.wait_until_gone("OPERATION", 5), "operation overlay closes")
        session.send(b"\x1b")  # clear search
        session.send(b"\x1b")  # back to menu
        check(session.wait_for("MAIN MENU", 10), "returns to the main menu")

        # AUR metadata over the network.
        session.send(b"\r")
        session.wait_for("INSTALL PACKAGES", 10)
        for character in "spotify":  # AUR-only package
            session.send(character.encode(), settle=0.05)
        aur_info = session.wait_for("Maintainer", 45)
        check(aur_info, "AUR metadata loads in the info pane")
        session.save_screen("05-aur-info")
        if not aur_info:
            print("---- current screen ----")
            print("\n".join(session.screen_text().split("\n")[20:]))
            print("------------------------")
        session.send(b"\x1b")
        session.send(b"\x1b")
        check(session.wait_for("MAIN MENU", 10), "back to menu again")

        if not sudo_available:
            # System Tools -> Refresh databases -> confirm -> sudo -n fails.
            for _ in range(6):
                session.send(b"j", settle=0.05)
            session.send(b"\r")
            check(session.wait_for("SYSTEM TOOLS", 10), "system tools screen opens")
            session.send(b"\r")
            check(session.wait_for("CONFIRM", 10), "maintenance action asks for confirmation")
            session.send(b"y")
            check(session.wait_for("OPERATION", 10), "privileged command starts")
            failed = False
            deadline = time.time() + 30
            while time.time() < deadline and not failed:
                session.drain(0.25)
                text = session.screen_text()
                failed = "failed" in text or "sudo:" in text or "not allowed" in text
            check(failed, "sudo failure is reported in the operation console")
            session.save_screen("06-privileged-failure")
            session.send(b"\x1b")
            session.send(b"\x1b")
            session.wait_for("MAIN MENU", 10)
        else:
            print("skip privileged failure check: sudo -n is available")

        session.send(b"q")
        status = session.close()
        check(status == 0, f"process exits cleanly (code {status})")
    finally:
        if session.proc.poll() is None:
            session.proc.kill()
        os.makedirs(os.path.join(ROOT, ".dev"), exist_ok=True)
        transcript = os.path.join(ROOT, ".dev", "pty-transcript.log")
        with open(transcript, "wb") as handle:
            handle.write(bytes(session.raw))
        print(f"transcript: {transcript}")
        print(f"screens: {os.path.join(ROOT, '.dev', 'pty-screens')}")

    if failures:
        print(f"\n{len(failures)} check(s) failed")
        return 1
    print("\nall PTY checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())

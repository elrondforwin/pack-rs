#!/usr/bin/env python3
"""Linux TUI smoke tests with fake managers. Never runs real package operations.

Run: cargo build --locked && python3 tests/smoke.py [target/debug/pack]
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import sys
import tempfile
import termios
import time

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/pack").resolve()
FAKE = r'''#!/usr/bin/python3
import json, os, subprocess, sys
from pathlib import Path
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ["PACK_TEST_LOG"], "a") as log:
    log.write(json.dumps([name, *args]) + "\n")
if name == "id": print("1000")
elif name == "sudo": sys.exit(subprocess.call(args))
elif "--info" in args or "-Qi" in args or "-Sii" in args: print("Name : bash\nDescription : Test package")
elif name == "dnf" and "repoquery" in args:
    query_format = args[args.index("--queryformat") + 1]
    rows = [("fedora", "bash", "x86_64", 9), ("fedora", "bash", "i686", 7),
            ("updates", "fzf", "noarch", 100), ("fedora", "bash", "aarch64", 6),
            ("fedora", "bash", "x86_64", 99)]
    if "--available" not in args:
        rows.append(("None" if os.environ["PACK_TEST_DNF_VERSION"] == "4" else "", "local-app", "x86_64", 50))
    for repository, package, arch, installed_at in rows:
        record = query_format
        for tag, value in [("repoid", repository), ("from_repo", repository),
                           ("name", package), ("arch", arch), ("installtime", str(installed_at))]:
            record = record.replace("%{" + tag + "}", value)
        record = record.replace("\\n", "\n")
        # DNF5 prints exactly the requested format; DNF4 adds its own newline.
        sys.stdout.write(record + ("\n" if os.environ["PACK_TEST_DNF_VERSION"] == "4" else ""))
elif "-Sl" in args:
    print("core bash 1\nextra fzf 1")
    if name == "yay": print("aur demo 1")
elif "-Qq" in args or "-Qtdq" in args or "-Qqm" in args: print("bash\nfzf")
'''


class Session:
    def __init__(self, root, choice, installed, route="install", dnf_version=5,
                 architecture_blacklist=None, cache_entries=None, cache_age_days=0):
        self.root = root
        self.log = root / "calls.jsonl"
        self.output = b""
        bindir = root / "bin"
        bindir.mkdir()
        for name in [*installed, "sudo", "id"]:
            path = bindir / name
            path.write_text(FAKE)
            path.chmod(0o755)
        config = root / "config" / "pack"
        config.mkdir(parents=True)
        (config / "settings.toml").write_text(
            f'package_manager = "{choice}"\n[appearance]\ntheme = "default"\n'
            f'[fedora]\narchitecture_blacklist = {json.dumps(architecture_blacklist or [])}\n'
        )
        cache = root / "cache" / "pack"
        cache.mkdir(parents=True)
        # A fresh legacy list must never leak into a selected backend's cache.
        (cache / "cache").write_text("aur/legacy-only\n")
        # DNF5 previously emitted all records without separators into this file.
        (cache / "cache-dnf").write_text("bash.x86_64bash.i686fzf.noarch\n")
        (cache / "cache-dnf-v2").write_text("bash.x86_64\nfzf.noarch\n")
        if choice != "yay":
            (cache / "cache-yay").write_text("aur/leaked-from-yay\n")
        if cache_entries is not None:
            filename = "cache-dnf-v3" if choice == "dnf" else f"cache-{choice}"
            path = cache / filename
            path.write_text("".join(package + "\n" for package in cache_entries))
            modified = time.time() - cache_age_days * 24 * 60 * 60
            os.utime(path, (modified, modified))
        env = dict(os.environ, PATH=str(bindir), TERM="xterm-256color",
                   XDG_CONFIG_DIR=str(root / "config"), XDG_CACHE_HOME=str(root / "cache"),
                   PACK_TEST_LOG=str(self.log), PACK_TEST_DNF_VERSION=str(dnf_version))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(str(BINARY), [str(BINARY), *([route] if route else [])], env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def wait(self, predicate, label):
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if predicate():
                return
            if select.select([self.fd], [], [], 0.05)[0]:
                try:
                    data = os.read(self.fd, 65536)
                except OSError:
                    break
                self.output += data
                if b"\x1b[6n" in data:
                    os.write(self.fd, b"\x1b[1;1R")
        raise AssertionError(f"Timed out: {label}; calls={self.calls()}; output={self.output[-1500:]!r}")

    def send(self, keys):
        os.write(self.fd, keys)

    def close(self):
        os.kill(self.pid, signal.SIGTERM)
        os.waitpid(self.pid, 0)
        os.close(self.fd)


def install_check(root, choice, installed, expected, dnf_version=5):
    session = Session(root, choice, installed, dnf_version=dnf_version)
    try:
        session.wait(lambda: any("--info" in call or "-Sii" in call for call in session.calls()), "package details")
        filename = "cache-dnf-v3" if expected == "dnf" else f"cache-{expected}"
        cache = root / "cache" / "pack" / filename
        packages = cache.read_text().splitlines()
        assert "aur/legacy-only" not in packages
        assert "aur/leaked-from-yay" not in packages
        if expected == "dnf":
            assert packages == ["fedora/bash.x86_64", "fedora/bash.i686", "updates/fzf.noarch", "fedora/bash.aarch64"]
        else:
            assert ("aur/demo" in packages) == (expected == "yay")
        session.wait(lambda: b"Description:" in session.output and b"Test" in session.output, "rendered details")
        if expected == "dnf":
            assert b"fedora" in session.output and b"x86_64" in session.output
            assert any("--repo=fedora" in call and call[-1] == "bash.x86_64" for call in session.calls())
        session.send(b"\r")
        operation = "install" if expected == "dnf" else "-S"
        session.wait(lambda: any(call[:2] == [expected, operation] for call in session.calls()), "installation")
        calls = session.calls()
        if expected == "dnf":
            assert ["dnf", "install", "--", "bash.x86_64"] in calls
        assert all(call[0] in [expected, "id", "sudo"] for call in calls), calls
        assert any(call[0] == "sudo" for call in calls) == (expected != "yay")
        backend = f"dnf{dnf_version}" if expected == "dnf" else expected
        print(f"PASS install/cache/details: {choice} -> {backend}")
    finally:
        session.close()


def dnf_action_check(root, route, keys, expected):
    session = Session(root, "dnf", ["dnf"], route)
    try:
        session.wait(lambda: b"confirm" in session.output if route == "update" else b"Install Packages" in session.output, "initial menu")
        if keys:
            session.send(keys)
            session.wait(lambda: b"confirm" in session.output, "action confirmation")
        session.send(b"y")
        session.wait(lambda: any(call[:len(expected)] == expected for call in session.calls()), str(expected))
        assert any(call[:2] == ["sudo", "dnf"] for call in session.calls())
        print(f"PASS DNF action: {expected[1:]}")
    finally:
        session.close()


def dnf_list_check(root, menu_keys, expected_query, first_package="bash.x86_64", dnf_version=5):
    session = Session(root, "dnf", ["dnf"], "" if menu_keys else "remove", dnf_version=dnf_version,
                      architecture_blacklist=["i686", "aarch64"])
    try:
        if menu_keys:
            session.wait(lambda: b"Install Packages" in session.output, "main menu")
            session.send(menu_keys)
        session.wait(lambda: any(call[:len(expected_query)] == expected_query for call in session.calls()), "list query")
        session.wait(lambda: b"Description:" in session.output and b"Test" in session.output, "installed details")
        session.wait(lambda: b"@System" in session.output, "unknown installed repository")
        assert b"i686" in session.output and b"aarch64" in session.output
        session.send(b"\r")
        session.wait(lambda: b"confirm" in session.output, "removal confirmation")
        session.send(b"y")
        session.wait(lambda: any(call[:3] == ["dnf", "remove", "--"] for call in session.calls()), "removal")
        assert ["dnf", "remove", "--", first_package] in session.calls()
        assert any(call[:2] == ["sudo", "dnf"] for call in session.calls())
        print(f"PASS DNF list and removal: {expected_query[1:]}")
    finally:
        session.close()


def architecture_blacklist_check(root, mode):
    all_packages = ["fedora/bash.x86_64", "fedora/bash.i686", "updates/fzf.noarch", "fedora/bash.aarch64"]
    blacklist = ["i686", "aarch64"] if mode != "empty" else ["x86_64", "i686", "aarch64", "noarch"]
    session = Session(root, "dnf", ["dnf"], architecture_blacklist=blacklist,
                      cache_entries=all_packages, cache_age_days=0 if mode == "fresh" else 6)
    cache = root / "cache/pack/cache-dnf-v3"
    def available_queries():
        return [call for call in session.calls() if "--available" in call and "--queryformat" in call]
    try:
        if mode == "empty":
            session.wait(lambda: b"No packages found." in session.output, "empty filtered cache")
            assert cache.read_text() == ""
            assert len(available_queries()) == 1
        else:
            session.wait(lambda: b"Description:" in session.output and b"Test" in session.output, "blacklist package list")
            if mode == "fresh":
                assert cache.read_text().splitlines() == all_packages
                assert not available_queries()
                assert b"i686" in session.output and b"aarch64" in session.output
                # Change the config while Pack is running. Only a rebuild should apply it.
                (root / "config/pack/settings.toml").write_text(
                    'package_manager = "dnf"\n[appearance]\ntheme = "default"\n'
                    '[fedora]\narchitecture_blacklist = ["noarch"]\n'
                )
                session.output = b""
                session.send(b"\x1b")
                session.wait(lambda: b"Advanced Options" in session.output, "return to main menu")
                session.send(b"jjj\rj\r")
                session.wait(lambda: b"confirm" in session.output, "manual cache confirmation")
                session.send(b"y")
                expected = [package for package in all_packages if not package.endswith(".noarch")]
                session.wait(lambda: cache.read_text().splitlines() == expected, "manual refresh rereads blacklist")
                assert len(available_queries()) == 1
            else:
                assert cache.read_text().splitlines() == ["fedora/bash.x86_64", "updates/fzf.noarch"]
                assert len(available_queries()) == 1
        print(f"PASS architecture blacklist: {mode}")
    finally:
        session.close()


with tempfile.TemporaryDirectory(prefix="pack-smoke-") as temporary:
    base = Path(temporary)
    for index, (choice, installed, expected) in enumerate([
        ("dnf", ["dnf", "pacman", "yay"], "dnf"),
        ("pacman", ["dnf", "pacman", "yay"], "pacman"),
        ("yay", ["pacman", "yay"], "yay"),
    ]):
        root = base / str(index)
        root.mkdir()
        install_check(root, choice, installed, expected)
    for index, (route, keys, expected) in enumerate([
        ("update", b"", ["dnf", "upgrade", "--refresh"]),
        ("", b"jjj\r\r", ["dnf", "clean", "all"]),
    ], 3):
        root = base / str(index)
        root.mkdir()
        dnf_action_check(root, route, keys, expected)

    for index, (keys, query, first_package) in enumerate([
        (b"", ["dnf", "--quiet", "repoquery", "--installed"], "bash.x86_64"),
        (b"j\rjj\r", ["dnf", "--quiet", "repoquery", "--unneeded"], "bash.x86_64"),
        (b"j\rj\r", ["dnf", "--quiet", "repoquery", "--installed", "--queryformat",
                       "%{installtime}\t%{from_repo}\t%{name}\t%{arch}\\n"], "fzf.noarch"),
    ], 5):
        root = base / str(index)
        root.mkdir()
        dnf_list_check(root, keys, query, first_package)

    root = base / "dnf4"
    root.mkdir()
    install_check(root, "dnf", ["dnf"], "dnf", dnf_version=4)

    root = base / "dnf4-installed"
    root.mkdir()
    dnf_list_check(root, b"", ["dnf", "--quiet", "repoquery", "--installed"], dnf_version=4)

    for mode in ["fresh", "expired", "empty"]:
        root = base / f"blacklist-{mode}"
        root.mkdir()
        architecture_blacklist_check(root, mode)

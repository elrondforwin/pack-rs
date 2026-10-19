#!/usr/bin/env python3
"""Exercise real TUI mixed-provider flows with fake package managers only."""
from pathlib import Path
import tempfile

from smoke import Session


def mixed(root, backend, remove=False, fail_program=""):
    session = Session(root, backend, [backend, "flatpak"], route="remove" if remove else "install",
                      flatpak_enabled=True, fail_program=fail_program)
    try:
        session.wait(lambda: b"Description:" in session.output and b"Test" in session.output, "initial native list")
        # Native identity remains checked after narrowing the search to Flatpak apps.
        session.send(b"\t")
        session.output = b""
        session.send(b"org.example.Shared")
        session.wait(lambda: b"org.example.Shared" in session.output and any(call[:2] == ["flatpak", "info" if remove else "remote-info"] for call in session.calls()), "Flatpak search and details")
        session.send(b"\x1b[5~\t\t\t\r")
        session.wait(lambda: b"confirm" in session.output, "mixed transaction confirmation")
        session.send(b"y")
        if fail_program:
            session.wait(lambda: b"Remaining" in session.output and b"Completed:" in session.output, "partial failure reporting")
        else:
            session.wait(lambda: any(call[:3] == ["flatpak", "uninstall" if remove else "install", "--system"] for call in session.calls()), "system Flatpak transaction")
            session.wait(lambda: b"completed:" in session.output, "completed mixed transaction")
        calls = session.calls()
        native_operation = ("remove" if remove else "install") if backend == "dnf" else ("-Rns" if remove else "-S")
        native_target = ("bash.x86_64" if backend == "dnf" else "core/bash") if not remove else ("bash.x86_64" if backend == "dnf" else "bash")
        assert [backend, native_operation, "--", native_target] in calls, calls
        operations = [call for call in calls if call[0] == "flatpak" and call[1] in ["install", "uninstall"]]
        if fail_program == backend:
            assert not operations
        elif fail_program == "flatpak":
            assert len(operations) == 1
        elif remove:
            assert ["flatpak", "uninstall", "--user", "--", "app/org.example.Shared/x86_64/stable", "app/org.example.Shared/aarch64/beta"] in operations
            assert ["flatpak", "uninstall", "--system", "--", "app/org.example.Shared/x86_64/stable"] in operations
        else:
            for scope, remote, ref in [("--user", "flathub", "app/org.example.Shared/x86_64/stable"),
                                       ("--user", "testing", "app/org.example.Shared/aarch64/beta"),
                                       ("--system", "flathub", "app/org.example.Shared/x86_64/stable")]:
                assert ["flatpak", "install", scope, "--", remote, ref] in operations
        assert not any(call[:2] == ["sudo", "flatpak"] for call in calls)
        if not remove:
            records = (root / "cache/pack/cache-flatpak-v1").read_text().splitlines()
            assert len(records) == 3
            assert any(row.startswith("user\ttesting\tapp/org.example.Shared/aarch64/beta\t") for row in records)
        print(f"PASS mixed {backend}/Flatpak: {'remove' if remove else 'install'}; failure={fail_program or 'none'}")
    finally:
        session.close()


def standalone(root):
    session = Session(root, "auto", ["flatpak"], flatpak_enabled=True)
    try:
        session.wait(lambda: b"Description:" in session.output and b"application" in session.output, "Flatpak-only details")
        session.send(b"\r")
        session.wait(lambda: b"confirm" in session.output, "Flatpak-only confirmation")
        session.send(b"y")
        session.wait(lambda: any(call[:2] == ["flatpak", "install"] for call in session.calls()), "Flatpak-only installation")
        assert all(call[0] in ["flatpak", "id"] for call in session.calls())
        print("PASS Flatpak-only mode without a supported native manager")
    finally:
        session.close()


def disabled(root):
    session = Session(root, "dnf", ["dnf", "flatpak"])
    try:
        session.wait(lambda: b"Description:" in session.output, "disabled Flatpak native list")
        assert all(call[0] != "flatpak" for call in session.calls())
        assert not (root / "cache/pack/cache-flatpak-v1").exists()
        print("PASS Flatpak disabled: no command or cache access")
    finally:
        session.close()


def cached(root):
    session = Session(root, "dnf", ["dnf", "flatpak"], flatpak_enabled=True,
                      cache_entries=["fedora/bash.x86_64"], flatpak_cache_entries=[
                          "user\tcustom\tapp/org.cached.App/x86_64/beta\tCached App",
                      ])
    try:
        session.wait(lambda: b"Description:" in session.output, "cached combined list")
        assert not any(call[:2] == ["flatpak", "remote-ls"] for call in session.calls())
        session.output = b""
        session.send(b"org.cached.App")
        expected_info = ["flatpak", "remote-info", "--user", "--", "custom", "app/org.cached.App/x86_64/beta"]
        session.wait(lambda: expected_info in session.calls(), "cached origin and branch for details")
        session.send(b"\r")
        session.wait(lambda: b"confirm" in session.output, "cached application confirmation")
        session.send(b"y")
        expected_install = ["flatpak", "install", "--user", "--", "custom", "app/org.cached.App/x86_64/beta"]
        session.wait(lambda: expected_install in session.calls(), "cached ref installation")
        print("PASS Flatpak cache round trip: exact scope, remote, architecture, branch")
    finally:
        session.close()


def unavailable_remote(root):
    session = Session(root, "dnf", ["dnf", "flatpak"], flatpak_enabled=True, fail_program="flatpak-query")
    try:
        session.wait(lambda: b"Flatpak" in session.output and b"Description:" in session.output, "native packages with remote failure notice")
        session.send(b"\r")
        session.wait(lambda: ["dnf", "install", "--", "bash.x86_64"] in session.calls(), "native install despite remote failure")
        assert not (root / "cache/pack/cache-flatpak-v1").exists()
        print("PASS unavailable Flatpak remote does not block native packages")
    finally:
        session.close()


def main():
    with tempfile.TemporaryDirectory(prefix="pack-flatpak-smoke-") as temporary:
        base = Path(temporary)
        cases = [(backend, remove, "") for backend in ["dnf", "pacman", "yay"] for remove in [False, True]]
        cases += [("dnf", False, "dnf"), ("dnf", False, "flatpak")]
        for index, (backend, remove, failure) in enumerate(cases):
            root = base / str(index)
            root.mkdir()
            mixed(root, backend, remove, failure)
        for name, check in [("standalone", standalone), ("disabled", disabled),
                            ("cached", cached), ("unavailable", unavailable_remote)]:
            root = base / name
            root.mkdir()
            check(root)


if __name__ == "__main__":
    main()

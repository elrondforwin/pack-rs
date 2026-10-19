#!/usr/bin/env python3
"""Build an RPM from this checkout without installing it or changing system packages."""

from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def collect_licenses(source):
    destination = source / "LICENSES.dependencies"
    destination.mkdir()
    entries = []
    for crate in sorted((source / "vendor").iterdir()):
        metadata = tomllib.loads((crate / "Cargo.toml").read_text())["package"]
        entries.append(
            f"{metadata['name']} {metadata['version']}: "
            f"{metadata.get('license', 'see license files')}"
        )
        notices = {
            path for path in crate.iterdir()
            if path.is_file()
            and path.name.upper().startswith(("LICENSE", "COPYING", "NOTICE", "COPYRIGHT"))
        }
        if license_file := metadata.get("license-file"):
            path = crate / license_file
            if path.is_file():
                notices.add(path)
        if notices:
            directory = destination / crate.name
            directory.mkdir()
            for path in sorted(notices):
                shutil.copy2(path, directory / path.name)
    (destination / "README.txt").write_text(
        "License notices for vendored Rust dependencies, including target-specific crates.\n\n"
        + "\n".join(entries) + "\n"
    )


def main():
    if len(sys.argv) != 1:
        raise SystemExit("Usage: python3 packaging/fedora/build.py")
    for command in ("cargo", "rustc", "cc", "rpmbuild", "rpmspec", "desktop-file-validate"):
        if shutil.which(command) is None:
            raise SystemExit(
                f"Missing {command}. Install the Fedora build dependencies listed in README.md."
            )

    root = Path(__file__).resolve().parents[2]
    metadata = tomllib.loads((root / "Cargo.toml").read_text())["package"]
    name, version = metadata["name"], metadata["version"]
    spec = root / "packaging/fedora/pack.spec"
    spec_version = run(
        "rpmspec", "-q", "--srpm", "--qf", "%{VERSION}", str(spec),
        capture_output=True,
    ).stdout.strip()
    if spec_version != version:
        raise SystemExit(f"Version mismatch: Cargo.toml has {version}, pack.spec has {spec_version}.")

    output = root / "target/rpm"
    for directory in ("BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"):
        (output / directory).mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="pack-rpm-") as temporary:
        source = Path(temporary) / f"{name}-{version}"
        source.mkdir()
        # Include current local edits, but never archive .git, caches, or build artifacts.
        for filename in (
            "Cargo.toml", "Cargo.lock", "LICENSE", "README.md", "pack.desktop",
            "packaging/arch/PKGBUILD", "packaging/fedora/pack.spec", "packaging/fedora/build.py",
        ):
            (source / filename).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(root / filename, source / filename)
        for directory in ("src", "tests"):
            shutil.copytree(root / directory, source / directory, ignore=shutil.ignore_patterns("__pycache__"))

        print("Preparing locked dependencies (internet access may be needed)...", flush=True)
        config = run(
            "cargo", "vendor", "--locked", "--versioned-dirs", "vendor",
            cwd=source, stdout=subprocess.PIPE,
        ).stdout
        (source / ".cargo").mkdir()
        (source / ".cargo/config.toml").write_text(config)
        collect_licenses(source)
        with tarfile.open(output / "SOURCES" / f"{name}-{version}.tar.gz", "w:gz") as archive:
            archive.add(source, arcname=source.name)

    staged_spec = output / "SPECS/pack.spec"
    shutil.copy2(spec, staged_spec)
    print("Building and testing the RPM offline...", flush=True)
    run("rpmbuild", "-bb", "--define", f"_topdir {output}", str(staged_spec), cwd=root)
    relative_rpm = run(
        "rpmspec", "-q", "--rpms", "--qf", "%{ARCH}/%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}.rpm",
        str(staged_spec), capture_output=True,
    ).stdout.strip()
    package = output / "pack.rpm"
    shutil.copy2(output / "RPMS" / relative_rpm, package)
    print(f"\nBuilt: {package}\nInstall with:\n  {shlex.join(['sudo', 'dnf', 'install', str(package)])}")


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.returncode) from error

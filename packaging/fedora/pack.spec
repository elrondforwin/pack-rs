# This recipe builds the local source archive prepared by build.py.
%global debug_package %{nil}

Name:           pack
Version:        0.1.0
Release:        1%{?dist}
Summary:        A terminal package manager for Arch Linux and Fedora
License:        GPL-3.0-only
URL:            https://github.com/elrondforwin/pack-rs
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.88
BuildRequires:  rust >= 1.88
BuildRequires:  gcc
BuildRequires:  desktop-file-utils
BuildRequires:  python3 >= 3.11
Requires:       /usr/bin/dnf
Requires:       coreutils
Requires:       sudo

%description
Pack provides an interactive terminal interface for searching, installing,
removing, updating, and inspecting packages. On Fedora it uses DNF and RPM.

%prep
%setup -q

%build
export CARGO_HOME="$PWD/.cargo-home"
export CARGO_TARGET_DIR="$PWD/target"
export RUSTFLAGS="%{?build_rustflags}"
cargo build --release --frozen

%install
install -Dm755 target/release/pack "%{buildroot}%{_bindir}/pack"
install -Dm644 pack.desktop "%{buildroot}%{_datadir}/applications/pack.desktop"

%check
export CARGO_HOME="$PWD/.cargo-home"
export CARGO_TARGET_DIR="$PWD/target"
export RUSTFLAGS="%{?build_rustflags}"
cargo test --release --frozen
python3 tests/smoke.py target/release/pack
desktop-file-validate pack.desktop

%files
%license LICENSE LICENSES.dependencies
%doc README.md
%{_bindir}/pack
%{_datadir}/applications/pack.desktop

%changelog
* Sun Oct 18 2026 Pack contributors - 0.1.0-1
- Add a local Fedora RPM build.

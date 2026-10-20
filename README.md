# Pack

Pack is a lightweight terminal package manager for Arch Linux and Fedora with optional Flatpak support, written in Rust with [Ratatui](https://github.com/ratatui/ratatui).

It provides an interactive interface for searching, installing, removing, updating, and inspecting packages through `pacman`, `yay`, or `dnf`.

## Requirements

- Arch Linux with `pacman`, or Fedora with `dnf` (DNF4 or DNF5)
- `sudo` for privileged pacman/DNF operations when running as a regular user
- Optional on Arch: `yay` for AUR support and `expac` for recently installed packages
- Optional on any Linux distribution: `flatpak` and configured remotes.

## Configuration

Pack creates `settings.toml` in `${XDG_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}}/pack/` on first launch:

```toml
# Top-level key: place this before [appearance].
package_manager = "auto"

[appearance]
theme = "default"
details_visible = true

[fedora]
# Fedora/DNF only. Example: ["i686", "aarch64"]
# Applied when Pack's install-package cache is refreshed, not while browsing.
architecture_blacklist = []

[flatpak]
# Optional on every distribution; requires flatpak and configured remotes.
enabled = false
```

Choose `auto`, `pacman`, `yay`, or `dnf` for `package_manager`:

- `auto`: prefer yay/pacman on Arch and DNF on Fedora; otherwise use an available supported manager.
- `pacman`: use configured pacman repositories without AUR.
- `yay`: include AUR support.
- `dnf`: use enabled DNF repositories.

An explicitly selected manager must be installed. Restart Pack after changing the
manager, theme, details visibility, or Flatpak setting.

Set `[appearance].details_visible = false` to open package lists with details
hidden. Ctrl+P toggles the panel; the default is `true`.

Fedora's `architecture_blacklist` excludes exact architectures from the
install-package cache. For example, `["i686", "aarch64"]` excludes those
architectures; an empty list allows all. Installed-package lists are unaffected.
Changes take effect on the next cache refresh without restarting Pack.

Package lists are cached separately for each backend for five days. Use
**Advanced Options → Refresh Pack Cache** to rebuild them sooner.
**Clear Package Manager Cache** clears the native manager's own cache.

## Optional Flatpak support

Set `[flatpak].enabled = true` and restart Pack. Flatpak is disabled by default;
Pack does not install it or add remotes automatically.

Applications appear alongside native packages in **Install Packages** and
**All Installed Packages**, showing the remote, name, architecture, branch, and
`flatpak:user` or `flatpak:system` scope. User and default system installations
are supported; named system installations are excluded. Recent, AUR, and orphan
lists contain native packages only.

You can select native and Flatpak packages together. Pack shows a plan before
Flatpak installs and sends each selection to its package manager. If a command
fails, completed operations remain applied and the remaining commands stop.

**Update System** includes Flatpak updates, and **Refresh Pack Cache** refreshes
both catalogs. An unavailable Flatpak remote does not prevent browsing native
packages; Pack displays a notice if it uses a stale Flatpak catalog.

With `package_manager = "auto"`, Flatpak-only mode is available on distributions
without pacman, yay, or DNF.

## Installation

### Cargo (common installation method)

On any supported Linux distribution, install from the checkout as your regular
user. Requires Rust 1.88 or newer, Cargo, and a C compiler/linker:

```bash
git clone https://github.com/elrondforwin/pack-rs.git
cd pack-rs
cargo install --path . --locked
```

Cargo installs `pack` in `~/.cargo/bin` by default (`$CARGO_HOME/bin` when set).
No `sudo` is needed. Add that directory to your shell's `PATH` if necessary:

```bash
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
pack
```

Persist the `export` in `~/.zshrc` or `~/.bashrc`. If multiple copies are installed,
`command -v pack` shows which one your shell launches.

To update, run these commands from the checkout:

```bash
git pull --ff-only
cargo install --path . --locked
```

Cargo installs the binary only. Install runtime requirements separately; use the
Arch or RPM package below if you also want a desktop launcher.

### Arch package

From the repository root, build and install the local package with:

```bash
makepkg -D packaging/arch -si
```

### Fedora RPM

Clone the repository, install the build tools, then build and install a local RPM:

```bash
git clone https://github.com/elrondforwin/pack-rs.git
cd pack-rs
sudo dnf install cargo rust gcc rpm-build desktop-file-utils python3
python3 packaging/fedora/build.py
sudo dnf install ./target/rpm/pack.rpm
```

The RPM helper requires Python 3.11 or newer and the same Rust version as the
Cargo installation. Run it as your regular user. The RPM installs the binary and
desktop launcher; DNF resolves runtime dependencies.

To rebuild after updating the checkout, rerun the build and install commands.
If the same version is already installed, use:

```bash
sudo dnf reinstall ./target/rpm/pack.rpm
```

## Usage

Launch the interactive interface:

```bash
pack
```

## Themes

In the configuration directory above, Pack creates `themes/default.toml` on first
launch. Copy it to `themes/mytheme.toml`, edit its colors, and select it in
`settings.toml`:

```toml
[appearance]
theme = "mytheme"
```

The theme's `[colors]` section has `selected`, `text`, `muted`, `separator`, and `search` keys. Colors accept names such as `cyan`, palette indexes such as `212`, and hex values such as `"#ff88cc"`. Restart Pack to load changes.

## Uninstallation

If installed with Cargo:

```bash
cargo uninstall pack
```

This removes the Cargo-managed binary and keeps your configuration and cache.

If installed with makepkg:

```bash
sudo pacman -Rns pack
```

If installed as a Fedora RPM:

```bash
sudo dnf remove pack
```

Configuration and cache are kept when uninstalling. Pack's cache directory is
`${XDG_CACHE_HOME:-$HOME/.cache}/pack/`.

## Development checks

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/smoke.py
python3 tests/flatpak_smoke.py
```

The smoke tests require Python 3 at `/usr/bin/python3`. They use fake package
managers and do not modify system packages.

## Credits

Pack’s interaction model and visual direction were inspired by:

- [fzf](https://github.com/junegunn/fzf) -> fuzzy finding, reverse lists, pointers, and selection behavior.
- [gum](https://github.com/charmbracelet/gum) -> simple terminal menus and color-focused styling.
- [skim](https://github.com/lotabout/skim) -> Rust-based fuzzy finder patterns and architecture reference.

Pack is built with:

- [Ratatui](https://github.com/ratatui/ratatui) -> terminal user interface framework.
- [crossterm](https://github.com/crossterm-rs/crossterm) -> terminal input, mouse events, and screen control.

## License

Pack is released under the [GNU General Public License v3.0](LICENSE).

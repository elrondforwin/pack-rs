# Pack

Pack is a lightweight terminal package manager for Arch Linux and Fedora with optional Flatpak support, written in Rust with [Ratatui](https://github.com/ratatui/ratatui).

It provides an interactive interface for searching, installing, removing, updating, and inspecting packages through `pacman`, `yay`, or `dnf`.

## Requirements

- Arch Linux with `pacman`, or Fedora with `dnf` (DNF4 or DNF5)
- `sudo` for privileged pacman/DNF operations when running as a regular user
- Optional on Arch: `yay` for AUR support and `expac` for recently installed packages
- On Fedora: DNF's built-in `repoquery` command supplies package lists and metadata
- Optional on any Linux distribution: `flatpak` and configured remotes. When enabled, Pack also works in Flatpak-only mode if no supported native package manager is found.

## Package manager selection

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

`package_manager` accepts `auto`, `pacman`, `yay`, or `dnf`. Existing settings without this key use `auto` and keep their theme. Restart Pack after changing it.

- `auto`: on Arch, choose `yay` if it is executable on `PATH`, otherwise `pacman`. On Fedora and Fedora/RHEL derivatives, prefer `dnf`. When the native choice is unavailable or the distribution is unknown, fall back to available commands in `yay`, `pacman`, `dnf` order (DNF first on Fedora/RHEL).
- `pacman`: use only official/configured pacman repositories, even if yay is installed. AUR packages are excluded from Pack's install cache and the AUR menu is hidden.
- `yay`: enable AUR operations. Requires `yay` on `PATH`.
- `dnf`: use enabled DNF repositories. Requires `dnf` on `PATH`.

An explicitly selected manager that is missing produces an error; Pack does not silently switch managers.

Set `[appearance].details_visible = false` to open package lists with details hidden.
Ctrl+P still toggles the details panel. Missing settings default to `true`; restart
Pack after changing this option.

Fedora's `architecture_blacklist` excludes exact architecture names when building
the install-package cache. For example, `architecture_blacklist = ["i686", "aarch64"]`
prevents those packages from being cached; the default empty list allows all
architectures. This setting is ignored by pacman/yay and does not hide installed,
recent, or unneeded packages. Existing settings without the key allow all
architectures and are left unchanged.

Changing the blacklist does not invalidate or filter an existing cache. It takes
effect on the next automatic refresh after the five-day TTL or through
**Advanced Options → Refresh Pack Cache**. The configuration is reread only when
rebuilding the DNF cache, so manual refresh applies edits without restarting Pack.

Fedora supports package installation/removal, system upgrades, available and installed lists, package details, the 200 most recently installed packages, unneeded dependencies, and package manager cache cleaning. DNF lists show `repository › name architecture`, with the repository and architecture in the same muted theme color. AUR is exclusive to yay; it has no Fedora equivalent. DNF removal follows DNF's dependency/configuration handling, rather than reproducing pacman's `-Rns` semantics.

Repository metadata is fetched in the same bulk `dnf repoquery` call as package
names and architectures. Available packages use `%{repoid}`; installed, recent,
and unneeded packages use `%{from_repo}` (the recorded installation origin).
When DNF has no origin recorded, Pack shows `@System`. Recent packages also
request `%{installtime}` and are sorted newest first. Records use tab-separated
fields and an explicit newline so DNF4 and DNF5 both parse correctly. Package
operations keep the `name.arch` identifier; available-package details are scoped
to the displayed repository.

Advanced Options contains one **Refresh Pack Cache** action, which uses the selected manager, and **Clear Package Manager Cache** (`pacman/yay -Scc` or `dnf clean all`). There is no separate “without AUR” refresh; select `pacman` for that behavior.

Package lists are cached separately as `cache-pacman`, `cache-yay`, and `cache-dnf-v3`. Switching managers cannot reuse an AUR or another backend's list. The legacy `cache`, `cache-dnf`, and `cache-dnf-v2` files are ignored and may be deleted; earlier DNF caches lack repository metadata or contain concatenated package names. Pack's list cache lasts five days; use Refresh Pack Cache to rebuild it sooner. This is separate from each package manager's own metadata/download cache.

## Optional Flatpak support

Set `[flatpak].enabled = true` in `settings.toml` and restart Pack. The default is
`false`: Pack does not run Flatpak commands or read its catalog when disabled.
Enabling it requires the `flatpak` command on `PATH`; Pack does not install Flatpak
or add remotes automatically.

Flatpak applications appear alongside native packages in **Install Packages** and
**All Installed Packages**. Rows show the origin remote, application name,
architecture, and a muted `flatpak:user` or `flatpak:system` label with the branch.
The user and default system installations are queried separately; named system
installations are not included. Recent, AUR, and orphan lists remain native-only.

For available applications Pack reads `origin`, `ref`, and `name` from
`flatpak remote-ls --app`; installed applications use the same columns from
`flatpak list --app`. Both commands specify `--user` or `--system` and full-width
columns. Remote names come directly from the `origin` field, not from guessed
URLs or application names. Full application refs retain the application ID,
architecture, and branch. Runtimes are managed by Flatpak as dependencies.

Native and Flatpak entries can be selected together. Pack shows the transaction
plan before Flatpak installs, sends native selections to the chosen native
manager, and groups Flatpak installs by installation and origin remote. Removals
use the installation and exact ref. Flatpak runs as the current user, using its
normal authorization for system installations. If a command fails, Pack reports
completed commands and stops the remaining ones; changes across managers are not
atomic.

**Update System** includes Flatpak updates when enabled. **Refresh Pack Cache**
refreshes both catalogs; Flatpak's catalog has its own five-day `cache-flatpak-v1`
file. Fedora's architecture blacklist applies only to the native DNF cache.
An unavailable Flatpak remote does not prevent browsing native packages; Pack
shows the error or a notice when using a stale Flatpak catalog.

With `package_manager = "auto"`, enabling Flatpak also permits Flatpak-only mode
on distributions without pacman, yay, or DNF. An explicitly configured native
manager must still be available.

## Installation

### Cargo (common installation method)

On any supported Linux distribution, install from the checkout as your regular
user. Requires Rust 1.88 or newer, Cargo, and a C compiler/linker:

```bash
git clone https://github.com/elrondforwin/pack-rs.git
cd pack-rs
cargo install --path . --locked
```

Cargo builds the release binary and installs `pack` in `~/.cargo/bin` by default
(or `$CARGO_HOME/bin` when set, unless a Cargo installation-root override is
configured). No `sudo` is needed. Add the Cargo bin directory to your shell's
`PATH` if necessary:

```bash
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
pack
```

Persist that `export` in your shell configuration, such as `~/.zshrc` or
`~/.bashrc`. If an RPM or manually installed copy also exists, `command -v pack`
shows which binary your shell will launch; put Cargo's bin directory first to
use the Cargo version.

To update, run these commands from the checkout:

```bash
git pull --ff-only
cargo install --path . --locked
```

The same install command also rebuilds local edits without changing the version.
Cargo installs the binary only: install the native package manager, optional
Flatpak, and other runtime requirements separately. Pack creates its user
configuration on first launch; Cargo does not install a desktop launcher.
RPM and Arch packages below provide desktop integration and package-manager
tracking instead. See the [Cargo installation reference](https://doc.rust-lang.org/cargo/commands/cargo-install.html)
for installation-root overrides.

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

The locked dependencies currently require Rust 1.88 or newer and the build helper
requires Python 3.11 or newer. Run the build as your regular user. The helper
packages the current checkout, including local source edits, and prepares vendored
dependencies with `cargo vendor --locked`. This preparation may use the internet;
the RPM compilation and tests run offline with `--frozen`.

The resulting RPM installs `pack` in `/usr/bin`, its desktop entry, and license
notices. DNF resolves its runtime dependencies. Build files stay in `target/rpm/`;
no COPR account or additional repository is required. To rebuild after updating
the checkout, rerun the last two commands.
If the same package version is already installed, use
`sudo dnf reinstall ./target/rpm/pack.rpm` to install the rebuilt binary.

Distribution recipes live in `packaging/arch/` and `packaging/fedora/`.

### Manual binary installation

```bash
cargo build --release --locked
install -Dm755 target/release/pack ~/.local/bin/pack
install -Dm644 pack.desktop ~/.local/share/applications/pack.desktop
```

Make sure `~/.local/bin` is in your `PATH`.

## Usage

Launch the interactive interface:

```bash
pack
```

## Themes

On first launch, Pack creates `settings.toml` and `themes/default.toml` in `${XDG_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}}/pack/`.
To add a theme, copy `themes/default.toml` to `themes/mytheme.toml`, edit its colors, and select it in `settings.toml`:

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

If installed manually:

```bash
rm -f ~/.local/bin/pack
rm -f ~/.local/share/applications/pack.desktop
```

Pack's cache can be removed separately:

```bash
rm -f "${XDG_CACHE_HOME:-$HOME/.cache}/pack/"{cache,cache-pacman,cache-yay,cache-dnf,cache-dnf-v2,cache-dnf-v3,cache-flatpak-v1}
```

## Development checks

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
python3 tests/smoke.py
python3 tests/flatpak_smoke.py
```

The Linux smoke tests use a pseudo-terminal and fake package managers in a temporary directory. They exercise installation, removal, details, cache separation and migration, DNF4/DNF5 query formatting, DNF upgrade/clean, recent packages, and unneeded dependencies without modifying system packages. They require Python 3 at `/usr/bin/python3` and complement, rather than replace, testing on a real Fedora system.

The Flatpak smoke tests additionally cover mixed native/Flatpak selections for
pacman, yay, and DNF, origin and installation routing, exact refs, partial failures,
Flatpak-only mode, and the disabled default. All transactions use fake commands.

DNF command behavior follows the [DNF4 command reference](https://dnf.readthedocs.io/en/stable/command_ref.html) and [DNF5 repoquery reference](https://dnf5.readthedocs.io/en/latest/commands/repoquery.8.html).

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

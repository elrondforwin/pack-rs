# Pack

Pack is a lightweight terminal package manager for Arch Linux, written in Rust with [Ratatui](https://github.com/ratatui/ratatui).

It provides a simple interactive interface for searching, installing, removing, updating, and inspecting packages through `pacman`. If [yay](https://github.com/Jguer/yay) is available, it is used for AUR-aware operations; otherwise Pack falls back to `pacman`.

## Requirements

- Arch Linux
- `pacman`
- Optional: `yay` for AUR support
- Optional: `expac` for recently installed packages

## Installation

### Arch package

Build and install the local package with:

```bash
makepkg -si
```

### From source

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

## Uninstallation

If installed with `makepkg -si`:

```bash
sudo pacman -Rns pack
```

If installed manually:

```bash
rm -f ~/.local/bin/pack
rm -f ~/.local/share/applications/pack.desktop
```

Pack's cache can be removed separately:

```bash
rm -f "${XDG_CACHE_HOME:-$HOME/.cache}/pack/cache"
```

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

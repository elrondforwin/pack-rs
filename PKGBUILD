pkgname=pack
pkgver=0.1.0
pkgrel=1
pkgdesc="A Ratatui-based Arch Linux package manager"
arch=('x86_64')
url="https://github.com/elrondforwin/pack"
license=('GPL-3.0-only')
depends=('glibc' 'pacman')
makedepends=('cargo')
optdepends=(
  'yay: optional helper for AUR packages'
  'expac: list recently installed packages'
)
source=()
sha256sums=()

build() {
  cd "$startdir"
  cargo build --release --locked
}

package() {
  cd "$startdir"
  install -Dm755 target/release/pack "$pkgdir/usr/bin/pack"
  install -Dm644 pack.desktop "$pkgdir/usr/share/applications/pack.desktop"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}

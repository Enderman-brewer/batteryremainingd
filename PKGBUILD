# Maintainer: Zack <zack@example.com>
pkgname=batteryremainingd
pkgver=2.2.0
pkgrel=1
pkgdesc="Lightweight battery time remaining daemon with Unix socket API"
arch=('x86_64')
url=""
license=('MIT')
depends=('glibc')
makedepends=('cargo')
install=batteryremainingd.install
source=("$pkgname-$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
    cd "$srcdir/$pkgname-$pkgver"
    cargo build --release
}

package() {
    cd "$srcdir/$pkgname-$pkgver"
    install -Dm755 target/release/$pkgname "$pkgdir/usr/bin/$pkgname"
    install -Dm644 batteryremainingd.service \
        "$pkgdir/usr/lib/systemd/system/$pkgname.service"
}

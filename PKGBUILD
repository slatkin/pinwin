# Maintainer: slatkin@woims.net
pkgname=pinwin
pkgver=0.4.1
pkgrel=1
pkgdesc="Layer-shell app host: runs an app in a panel at one edge of a Wayland output"
arch=('x86_64')
url="https://github.com/slatkin/pinwin"
license=('GPL3')
depends=('fontconfig' 'libxkbcommon')
source=("${pkgname}-${pkgver}-linux-x86_64.tar.gz::https://github.com/slatkin/${pkgname}/releases/download/v${pkgver}/${pkgname}-${pkgver}-linux-x86_64.tar.gz")
sha256sums=('0000000000000000000000000000000000000000000000000000000000000000')

package() {
    cd "${pkgname}-${pkgver}"
    install -Dm755 "pinwin" "${pkgdir}/usr/bin/pinwin"
    install -Dm644 "LICENSE" "${pkgdir}/usr/share/licenses/${pkgname}/LICENSE"
}

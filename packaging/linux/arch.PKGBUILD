pkgname=plutarchpages
pkgver=@VERSION@
pkgrel=1
pkgdesc='Read, organize, combine, split and secure PDFs'
arch=(@ARCH@)
url='https://github.com/yungSNAT/PlutarchPages'
license=('MIT OR Apache-2.0')
depends=(glibc gcc-libs hicolor-icon-theme libxkbcommon vulkan-icd-loader)
optdepends=(
  'wayland: native Wayland display support'
  'libx11: X11 and XWayland display support'
  'libxcursor: X11 cursor handling'
  'libxi: X11 input handling'
  'libxrandr: X11 display resolution handling'
  'libglvnd: GPU rendering through EGL'
  'mesa: GPU rendering through EGL'
  'vulkan-driver: GPU rendering through Vulkan'
  'xdg-desktop-portal: native desktop integration and file dialogs'
  'cups: printing support'
)
provides=("pdfcraft=@VERSION@")
conflicts=(pdfcraft)
# Stripped in package.sh, and strip here couldn't read the other architecture's binaries.
options=(!strip !debug)

package() {
  install -d "$pkgdir/usr" "$pkgdir/usr/share/licenses/$pkgname"
  cp -dr --no-preserve=ownership "$startdir/stage/bin" "$startdir/stage/share" "$pkgdir/usr/"
  find "$pkgdir/usr/share/doc/pdfcraft" -maxdepth 1 -type f \( -name 'LICENSE*' -o -name NOTICE -o -name 'OFL-*' \) \
    -exec cp -t "$pkgdir/usr/share/licenses/$pkgname" {} +
}

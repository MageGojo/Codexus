#!/usr/bin/env bash
# 在 macOS 上构建并打包 Codexus（Flutter GUI + Rust daemon）。
# 前置：Rust(stable) + Xcode + CocoaPods + Flutter(stable)。
# 用法（在仓库根目录运行）：  ./scripts/package-macos.sh
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "== 构建 Rust daemon (release) =="
(cd "$root/core" && cargo build --release -p ferry-daemon)

echo "== 构建 Flutter macOS 应用 (release) =="
(cd "$root/app" && flutter config --enable-macos-desktop >/dev/null && flutter pub get && flutter build macos --release)

app="$root/app/build/macos/Build/Products/Release/Codexus.app"
echo "== 把 daemon 打进 .app 并 ad-hoc 重签名 =="
cp "$root/core/target/release/ferry-daemon" "$app/Contents/Resources/ferry-daemon"
codesign --force -s - "$app/Contents/Resources/ferry-daemon"
codesign --force --deep -s - "$app"
codesign --verify --deep --strict "$app"

zip="$root/Codexus-macos.zip"
rm -f "$zip"
echo "== 打包 $zip =="
(cd "$(dirname "$app")" && ditto -c -k --keepParent "Codexus.app" "$zip")
echo "完成：$zip"

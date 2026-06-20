# 在 Windows 上构建并打包 Codexus（Flutter GUI + Rust daemon）。
# 前置：Rust(stable) + Visual Studio 2022 Desktop C++ + Flutter(stable)。
# 用法（在仓库根目录的 PowerShell 运行）：  .\scripts\package-windows.ps1
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot

Write-Host "== 构建 Rust daemon (release) ==" -ForegroundColor Cyan
Push-Location "$root/core"
cargo build --release -p ferry-daemon
Pop-Location

Write-Host "== 构建 Flutter Windows 应用 (release) ==" -ForegroundColor Cyan
Push-Location "$root/app"
flutter config --enable-windows-desktop | Out-Null
flutter pub get
flutter build windows --release
Pop-Location

$release = "$root/app/build/windows/x64/runner/Release"
Write-Host "== 把 daemon 拷到 GUI 同目录 ==" -ForegroundColor Cyan
Copy-Item "$root/core/target/release/ferry-daemon.exe" -Destination $release -Force

$zip = "$root/Codexus-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Write-Host "== 打包 $zip ==" -ForegroundColor Cyan
Compress-Archive -Path "$release/*" -DestinationPath $zip
Write-Host "完成：$zip" -ForegroundColor Green

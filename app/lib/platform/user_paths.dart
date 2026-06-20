import 'dart:io';

/// 跨平台解析用户主目录。
///
/// - 类 Unix（macOS / Linux）：`HOME`。
/// - Windows：`USERPROFILE`，再退到 `HOMEDRIVE` + `HOMEPATH`。
///
/// 与后端 Rust 侧 `home_dir()` 的解析顺序保持一致，保证 GUI 与 daemon 落在同一个
/// `~/.codexferry` 数据目录。
String? userHomeDir() {
  final env = Platform.environment;

  final home = env['HOME'];
  if (home != null && home.trim().isNotEmpty) {
    return home;
  }

  final profile = env['USERPROFILE'];
  if (profile != null && profile.trim().isNotEmpty) {
    return profile;
  }

  final drive = env['HOMEDRIVE'];
  final path = env['HOMEPATH'];
  if (drive != null &&
      drive.trim().isNotEmpty &&
      path != null &&
      path.trim().isNotEmpty) {
    return '$drive$path';
  }

  return null;
}

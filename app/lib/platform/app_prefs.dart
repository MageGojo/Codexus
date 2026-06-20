import 'dart:convert';
import 'dart:io';

import 'user_paths.dart';

/// 轻量本地偏好存储，落在 `~/.codexferry/gui_state.json`。
///
/// 刻意只用 `dart:io`，不引入额外原生插件，避免在缺少完整 Xcode 时影响
/// `flutter analyze` / `flutter test`，同时与后端 `~/.codexferry/` 目录约定保持一致。
class AppPrefs {
  AppPrefs({Directory? directory}) : _override = directory;

  final Directory? _override;

  /// 进程内只迁移一次（改名 CodeFerry -> CodexFerry 的 GUI 侧配套）。
  static bool _legacyDirMigrated = false;

  Directory get _baseDir {
    final override = _override;
    if (override != null) {
      return override;
    }
    final home = userHomeDir();
    final base = (home == null || home.trim().isEmpty)
        ? Directory.systemTemp.path
        : home;
    return Directory('$base/.codexferry');
  }

  File get _stateFile => File('${_baseDir.path}/gui_state.json');

  /// 把旧数据目录 `~/.codeferry` 整体改名为 `~/.codexferry`（与后端 daemon 同样的
  /// 迁移；谁先启动谁迁）。仅当新目录缺失且旧目录存在时执行，幂等、不丢数据。
  /// 测试用例传入 `directory` 覆盖时跳过。
  void _migrateLegacyDirIfNeeded() {
    if (_legacyDirMigrated || _override != null) {
      return;
    }
    _legacyDirMigrated = true;
    try {
      final home = userHomeDir();
      if (home == null || home.trim().isEmpty) {
        return;
      }
      final oldDir = Directory('$home/.codeferry');
      final newDir = Directory('$home/.codexferry');
      if (!newDir.existsSync() && oldDir.existsSync()) {
        oldDir.renameSync(newDir.path);
      }
    } catch (_) {
      // 迁移失败不影响功能：退化为在新目录重新开始。
    }
  }

  Future<Map<String, dynamic>> _read() async {
    _migrateLegacyDirIfNeeded();
    try {
      final file = _stateFile;
      if (!await file.exists()) {
        return <String, dynamic>{};
      }
      final content = await file.readAsString();
      if (content.trim().isEmpty) {
        return <String, dynamic>{};
      }
      final decoded = jsonDecode(content);
      return decoded is Map<String, dynamic> ? decoded : <String, dynamic>{};
    } catch (_) {
      return <String, dynamic>{};
    }
  }

  Future<void> _write(Map<String, dynamic> data) async {
    _migrateLegacyDirIfNeeded();
    final dir = _baseDir;
    if (!await dir.exists()) {
      await dir.create(recursive: true);
    }
    await _stateFile.writeAsString(jsonEncode(data));
  }

  Future<bool> isOnboardingDone() async {
    final data = await _read();
    return data['onboarding_done'] == true;
  }

  Future<void> setOnboardingDone({bool done = true}) async {
    final data = await _read();
    data['onboarding_done'] = done;
    await _write(data);
  }

  /// 主题模式：`system` / `light` / `dark`，默认跟随系统。
  Future<String> themeMode() async {
    final data = await _read();
    final value = data['theme_mode'];
    if (value is String &&
        (value == 'system' || value == 'light' || value == 'dark')) {
      return value;
    }
    return 'system';
  }

  Future<void> setThemeMode(String mode) async {
    final data = await _read();
    data['theme_mode'] = mode;
    await _write(data);
  }
}

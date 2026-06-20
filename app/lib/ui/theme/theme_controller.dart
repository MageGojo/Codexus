import 'package:flutter/material.dart';

import '../../platform/app_prefs.dart';

/// 主题控制器：持有当前 [ThemeMode] 并落盘到 `~/.codexferry/gui_state.json`。
///
/// 仅用 `ChangeNotifier`，不引入额外状态管理依赖；`CodexFerryApp` 监听它重建
/// `MaterialApp.themeMode`，侧边栏的主题切换按钮调用 [cycle]。
class ThemeController extends ChangeNotifier {
  ThemeController({AppPrefs? prefs, ThemeMode initial = ThemeMode.system})
    : _prefs = prefs ?? AppPrefs(),
      _mode = initial;

  final AppPrefs _prefs;
  ThemeMode _mode;

  ThemeMode get mode => _mode;

  static ThemeMode parse(String value) => switch (value) {
    'light' => ThemeMode.light,
    'dark' => ThemeMode.dark,
    _ => ThemeMode.system,
  };

  static String encode(ThemeMode mode) => switch (mode) {
    ThemeMode.light => 'light',
    ThemeMode.dark => 'dark',
    ThemeMode.system => 'system',
  };

  Future<void> set(ThemeMode mode) async {
    if (_mode == mode) {
      return;
    }
    _mode = mode;
    notifyListeners();
    await _prefs.setThemeMode(encode(mode));
  }

  /// 跟随系统 → 浅色 → 深色 → 跟随系统，循环切换。
  Future<void> cycle() {
    final next = switch (_mode) {
      ThemeMode.system => ThemeMode.light,
      ThemeMode.light => ThemeMode.dark,
      ThemeMode.dark => ThemeMode.system,
    };
    return set(next);
  }
}

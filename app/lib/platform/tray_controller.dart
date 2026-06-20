import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:tray_manager/tray_manager.dart';
import 'package:window_manager/window_manager.dart';

import 'sidecar_controller.dart';

/// 托盘 + 窗口生命周期控制器。
///
/// 作为菜单栏应用：点击窗口关闭按钮时隐藏到托盘而非退出，只有托盘“退出”
/// 菜单才真正销毁应用。
class AppTrayController with TrayListener, WindowListener {
  AppTrayController({required this.sidecar});

  final SidecarController sidecar;

  bool _quitting = false;

  Future<void> init() async {
    if (!Platform.isMacOS) {
      return;
    }
    try {
      windowManager.addListener(this);
      await windowManager.setPreventClose(true);
    } catch (_) {
      // 测试或无窗口环境下静默跳过。
    }
    try {
      await trayManager.setIcon('assets/tray_icon.png');
      await _setMenu();
      trayManager.addListener(this);
    } catch (_) {
      // 缺少托盘环境时静默跳过。
    }
  }

  Future<void> dispose() async {
    trayManager.removeListener(this);
    windowManager.removeListener(this);
    if (Platform.isMacOS) {
      try {
        await trayManager.destroy();
      } catch (_) {
        // ignore
      }
    }
  }

  /// 左键点击托盘图标即弹出菜单（菜单内含「显示界面 / 退出」等），更易发现。
  @override
  void onTrayIconMouseDown() {
    _popUpMenu();
  }

  /// 右键同样弹出菜单。
  @override
  void onTrayIconRightMouseDown() {
    _popUpMenu();
  }

  @override
  void onTrayMenuItemClick(MenuItem menuItem) {
    switch (menuItem.key) {
      case 'show':
        _showWindow();
      case 'start':
        sidecar.start();
      case 'stop':
        sidecar.stop();
      case 'quit':
        _quit();
    }
  }

  Future<void> _popUpMenu() async {
    if (!Platform.isMacOS) {
      return;
    }
    try {
      await trayManager.popUpContextMenu();
    } catch (_) {
      // 弹出失败时回退到直接显示窗口。
      await _showWindow();
    }
  }

  /// 关闭窗口时隐藏到托盘，保持后台代理常驻。
  @override
  void onWindowClose() {
    if (_quitting) {
      return;
    }
    _hideWindow();
  }

  Future<void> _setMenu() async {
    final menu = Menu(
      items: [
        MenuItem(key: 'show', label: '显示界面'),
        MenuItem.separator(),
        MenuItem(key: 'start', label: '启动后端'),
        MenuItem(key: 'stop', label: '停止后端'),
        MenuItem.separator(),
        MenuItem(key: 'quit', label: '退出'),
      ],
    );
    await trayManager.setContextMenu(menu);
  }

  Future<void> _showWindow() async {
    if (!Platform.isMacOS) {
      return;
    }
    try {
      await windowManager.show();
      await windowManager.focus();
    } catch (_) {
      // ignore
    }
  }

  Future<void> _hideWindow() async {
    if (!Platform.isMacOS) {
      return;
    }
    try {
      await windowManager.hide();
    } catch (_) {
      // ignore
    }
  }

  /// 真正退出：停止 sidecar，解除关窗拦截后销毁窗口。
  Future<void> _quit() async {
    _quitting = true;
    await sidecar.stop();
    try {
      await windowManager.setPreventClose(false);
      await windowManager.destroy();
    } catch (_) {
      exit(0);
    }
  }

  @visibleForTesting
  List<String> menuKeysForTest() => ['show', 'start', 'stop', 'quit'];
}

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import 'app/codexferry_app.dart';
import 'platform/app_prefs.dart';
import 'ui/theme/theme_controller.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await windowManager.ensureInitialized();

  if (Platform.isMacOS || Platform.isWindows || Platform.isLinux) {
    final options = WindowOptions(
      size: const Size(1180, 760),
      minimumSize: const Size(960, 660),
      center: true,
      title: 'Codexus',
      // macOS 隐藏原生标题栏（界面自绘红绿灯留白区）；Windows / Linux 保留原生标题栏。
      titleBarStyle:
          Platform.isMacOS ? TitleBarStyle.hidden : TitleBarStyle.normal,
    );
    windowManager.waitUntilReadyToShow(options, () async {
      await windowManager.show();
      await windowManager.focus();
    });
  }

  final prefs = AppPrefs();
  final mode = ThemeController.parse(await prefs.themeMode());
  runApp(
    CodexFerryApp(themeController: ThemeController(prefs: prefs, initial: mode)),
  );
}

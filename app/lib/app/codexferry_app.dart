import 'package:flutter/material.dart';

import '../ipc/ipc_client.dart';
import '../platform/sidecar_controller.dart';
import '../platform/tray_controller.dart';
import '../ui/pages/home_page.dart';
import '../ui/theme/app_theme.dart';
import '../ui/theme/theme_controller.dart';

/// 应用根:持有主题控制器、后端客户端、sidecar 与托盘控制器,直接进入主控制台
/// (已移除首启引导页)。
class CodexFerryApp extends StatefulWidget {
  const CodexFerryApp({super.key, this.themeController});

  final ThemeController? themeController;

  @override
  State<CodexFerryApp> createState() => _CodexFerryAppState();
}

class _CodexFerryAppState extends State<CodexFerryApp> {
  late final ThemeController _theme;
  final IpcClient _client = IpcClient();
  late final SidecarController _sidecar;
  late final AppTrayController _tray;

  @override
  void initState() {
    super.initState();
    _theme = widget.themeController ?? ThemeController();
    _theme.addListener(_onThemeChanged);
    _sidecar = SidecarController();
    _tray = AppTrayController(sidecar: _sidecar);
    _tray.init();
  }

  void _onThemeChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  @override
  void dispose() {
    _theme.removeListener(_onThemeChanged);
    _tray.dispose();
    _sidecar.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Codexus',
      debugShowCheckedModeBanner: false,
      themeMode: _theme.mode,
      theme: FerryTheme.light(),
      darkTheme: FerryTheme.dark(),
      home: HomePage(
        client: _client,
        sidecar: _sidecar,
        themeMode: _theme.mode,
        onCycleTheme: _theme.cycle,
      ),
    );
  }
}

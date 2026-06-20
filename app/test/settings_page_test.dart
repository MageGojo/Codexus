import 'package:app/ipc/ipc_client.dart';
import 'package:app/models/app_settings.dart';
import 'package:app/models/pool_snapshot.dart';
import 'package:app/ui/pages/settings_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _FakeSettingsClient extends IpcClient {
  _FakeSettingsClient() : super(baseUrl: 'http://127.0.0.1:0');

  AppSettings stored = const AppSettings(weatherCity: '上海');
  bool keyConfigured = false;
  String? savedStrategy;

  @override
  Future<SettingsResponse> getSettings() async => SettingsResponse(
    settings: stored,
    apizeroKeyConfigured: keyConfigured,
  );

  @override
  Future<SettingsResponse> saveSettings(AppSettings settings) async {
    stored = settings;
    return SettingsResponse(settings: settings, apizeroKeyConfigured: keyConfigured);
  }

  @override
  Future<void> setApizeroKey(String apiKey) async {
    keyConfigured = apiKey.isNotEmpty;
  }

  @override
  Future<PoolResponse> setPoolStrategy(String strategy) async {
    savedStrategy = strategy;
    return const PoolResponse(mode: 'pool', snapshot: PoolSnapshot());
  }
}

void main() {
  testWidgets('设置页渲染各分区并能保存', (tester) async {
    tester.view.physicalSize = const Size(1100, 1000);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = _FakeSettingsClient();
    await tester.pumpWidget(
      MaterialApp(home: Scaffold(body: SettingsPage(client: client))),
    );
    await tester.pumpAndSettle();

    // 三个分区标题。
    expect(find.text('apizero API Key'), findsOneWidget);
    expect(find.text('生活化点缀'), findsOneWidget);
    expect(find.text('账号池调度'), findsOneWidget);
    // 城市来自后端设置。
    expect(find.text('上海'), findsOneWidget);

    // 保存设置 -> 触发 setPoolStrategy（默认 quota_aware）。
    await tester.tap(find.text('保存设置'));
    await tester.pumpAndSettle();
    expect(client.savedStrategy, 'quota_aware');
  });
}

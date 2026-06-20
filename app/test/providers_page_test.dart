import 'package:app/ipc/ipc_client.dart';
import 'package:app/models/account_summary.dart';
import 'package:app/models/active_provider.dart';
import 'package:app/models/app_settings.dart';
import 'package:app/models/codex_status.dart';
import 'package:app/models/provider_preset.dart';
import 'package:app/models/session_record.dart';
import 'package:app/models/stats.dart';
import 'package:app/platform/sidecar_controller.dart';
import 'package:app/ui/pages/home_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _FakeIpcClient extends IpcClient {
  _FakeIpcClient() : super(baseUrl: 'http://127.0.0.1:0');

  @override
  Future<bool> health() async => true;

  @override
  Future<List<ProviderPreset>> providers() async => const [
    ProviderPreset(
      id: 'deepseek',
      name: 'DeepSeek',
      baseUrl: 'https://api.deepseek.com/v1',
      api: 'chat',
      defaultModel: 'deepseek-chat',
      apiKeyEnv: [],
      aliases: [],
      builtin: true,
    ),
    ProviderPreset(
      id: 'mycustom',
      name: 'My Custom',
      baseUrl: 'https://api.custom.test/v1',
      api: 'chat',
      defaultModel: 'custom-model',
      apiKeyEnv: [],
      aliases: [],
      builtin: false,
    ),
  ];

  @override
  Future<List<AccountSummary>> accounts() async => const [];

  @override
  Future<List<SessionRecord>> sessions({int limit = 50}) async => const [];

  @override
  Future<CodexStatus> codexStatus() async => const CodexStatus(
    home: '/tmp',
    configPath: '/tmp/.codex/config.toml',
    exists: false,
  );

  @override
  Future<ActiveProvider> activeProvider() async => const ActiveProvider(
    baseUrl: 'https://api.deepseek.com/v1',
    apiType: 'chat',
    defaultModel: 'deepseek-chat',
    apiKeyConfigured: false,
  );

  @override
  Future<Stats> stats({int days = 30}) async => Stats.empty;

  // 关闭生活化点缀，避免仪表盘在测试中发起天气/诗词网络调用。
  @override
  Future<SettingsResponse> getSettings() async => const SettingsResponse(
    settings: AppSettings(showWeather: false, showPoem: false),
    apizeroKeyConfigured: false,
  );
}

void main() {
  testWidgets('供应商页展示内置/自定义徽章与新增入口', (tester) async {
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    await tester.pumpWidget(
      MaterialApp(
        home: HomePage(client: _FakeIpcClient(), sidecar: SidecarController()),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.text('供应商'));
    await tester.pumpAndSettle();

    expect(find.text('新增自定义供应商'), findsOneWidget);
    expect(find.text('My Custom'), findsOneWidget);
    expect(find.text('内置'), findsWidgets);
    expect(find.text('自定义'), findsWidgets);
  });
}

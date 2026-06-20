import 'package:app/ipc/ipc_client.dart';
import 'package:app/ui/dialogs/add_account_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// 仅用于打开弹窗、切换分段的桩客户端（测试不触发任何网络提交）。
class _StubClient extends IpcClient {
  _StubClient() : super(baseUrl: 'http://127.0.0.1:0');
}

void main() {
  testWidgets('添加账号弹窗含多种方式且可切换', (tester) async {
    tester.view.physicalSize = const Size(1100, 900);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(() {
      tester.view.resetPhysicalSize();
      tester.view.resetDevicePixelRatio();
    });

    final client = _StubClient();
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => Center(
              child: ElevatedButton(
                onPressed: () => showAddAccountDialog(context, client),
                child: const Text('open'),
              ),
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.text('open'));
    await tester.pumpAndSettle();

    // 四种添加方式分段 + 默认 ChatGPT 主按钮。
    expect(find.text('ChatGPT'), findsOneWidget);
    expect(find.text('API Key'), findsOneWidget);
    expect(find.text('JSON'), findsOneWidget);
    expect(find.text('导入'), findsOneWidget);
    expect(find.text('用 ChatGPT 登录'), findsOneWidget);

    // 切到 API Key。
    await tester.tap(find.text('API Key'));
    await tester.pumpAndSettle();
    expect(find.text('保存 API Key'), findsOneWidget);

    // 切到 JSON：单个粘贴框 + 「解析并添加」主按钮（程序自动识别格式）。
    await tester.tap(find.text('JSON'));
    await tester.pumpAndSettle();
    expect(find.text('账号 JSON'), findsOneWidget);
    expect(find.text('解析并添加'), findsOneWidget);

    // 切到 导入：展示本机凭据路径。
    await tester.tap(find.text('导入'));
    await tester.pumpAndSettle();
    expect(find.text('~/.codex/auth.json'), findsOneWidget);
    expect(find.text('从本机导入'), findsOneWidget);
  });
}

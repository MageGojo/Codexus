import 'package:app/app/codexferry_app.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('CodexFerry app renders navigation shell', (tester) async {
    await tester.pumpWidget(const CodexFerryApp());

    await tester.pump();
    expect(find.text('仪表盘'), findsWidgets);
    expect(find.text('供应商'), findsOneWidget);
  });
}

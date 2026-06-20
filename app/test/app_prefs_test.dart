import 'dart:io';

import 'package:app/platform/app_prefs.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('AppPrefs 默认未完成引导，可写入并读回', () async {
    final dir = await Directory.systemTemp.createTemp('codexferry_prefs_test');
    addTearDown(() async {
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    });

    final prefs = AppPrefs(directory: dir);
    expect(await prefs.isOnboardingDone(), isFalse);

    await prefs.setOnboardingDone();
    expect(await prefs.isOnboardingDone(), isTrue);

    final reopened = AppPrefs(directory: dir);
    expect(await reopened.isOnboardingDone(), isTrue);

    await prefs.setOnboardingDone(done: false);
    expect(await prefs.isOnboardingDone(), isFalse);
  });
}

import 'dart:convert';
import 'dart:io';

import 'package:app/models/session_record.dart';
import 'package:app/platform/session_exporter.dart';
import 'package:flutter_test/flutter_test.dart';

SessionRecord _sample() {
  return SessionRecord(
    sessionId: 'sess-1',
    title: '帮我写快速排序',
    cwd: '/Users/demo/proj',
    updatedAt: DateTime.utc(2026, 6, 16, 10),
    inputTokens: 10,
    outputTokens: 20,
    totalTokens: 30,
  );
}

void main() {
  test('导出 JSON 含会话字段', () async {
    final dir = await Directory.systemTemp.createTemp('codexferry_export_json');
    addTearDown(() async {
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    });

    final path = await SessionExporter.exportJson([_sample()], directory: dir);
    final file = File(path);
    expect(await file.exists(), isTrue);

    final decoded = jsonDecode(await file.readAsString()) as Map<String, dynamic>;
    expect(decoded['count'], 1);
    final sessions = decoded['sessions'] as List<dynamic>;
    final first = sessions.first as Map<String, dynamic>;
    expect(first['session_id'], 'sess-1');
    expect(first['title'], '帮我写快速排序');
    expect(first['cwd'], '/Users/demo/proj');
    expect(first['total_tokens'], 30);
  });

  test('导出 Markdown 含标题与目录', () async {
    final dir = await Directory.systemTemp.createTemp('codexferry_export_md');
    addTearDown(() async {
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    });

    final path = await SessionExporter.exportMarkdown(
      [_sample()],
      directory: dir,
    );
    final content = await File(path).readAsString();
    expect(content, contains('# Codexus 会话导出'));
    expect(content, contains('帮我写快速排序'));
    expect(content, contains('/Users/demo/proj'));
  });
}

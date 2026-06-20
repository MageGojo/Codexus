import 'dart:convert';
import 'dart:io';

import '../models/session_record.dart';
import 'user_paths.dart';

/// 会话导出工具：把会话记录导出为 JSON / Markdown。
///
/// 用 `dart:io` 直接写入 `~/Downloads`（不可用时回退到 HOME / 临时目录），
/// 避免引入原生“另存为”面板插件，保证无完整 Xcode 时仍可分析与测试。
class SessionExporter {
  static Future<String> exportJson(
    List<SessionRecord> sessions, {
    Directory? directory,
  }) async {
    final payload = {
      'exported_at': DateTime.now().toIso8601String(),
      'count': sessions.length,
      'sessions': sessions.map(_toMap).toList(),
    };
    const encoder = JsonEncoder.withIndent('  ');
    return _writeFile('json', encoder.convert(payload), directory: directory);
  }

  static Future<String> exportMarkdown(
    List<SessionRecord> sessions, {
    Directory? directory,
  }) async {
    final buffer = StringBuffer()
      ..writeln('# Codexus 会话导出')
      ..writeln()
      ..writeln('- 导出时间：${DateTime.now().toIso8601String()}')
      ..writeln('- 记录数量：${sessions.length}')
      ..writeln();

    for (final s in sessions) {
      final title = s.title.isEmpty ? s.sessionId : s.title;
      buffer
        ..writeln('## $title')
        ..writeln()
        ..writeln('- 会话：${s.sessionId}')
        ..writeln('- 目录：${s.cwd}')
        ..writeln('- 更新：${s.updatedAt?.toIso8601String() ?? "-"}')
        ..writeln(
          '- 用量：${s.totalTokens} tokens（输入 ${s.inputTokens} / 输出 ${s.outputTokens}）',
        )
        ..writeln();
    }
    return _writeFile('md', buffer.toString(), directory: directory);
  }

  static Map<String, dynamic> _toMap(SessionRecord s) {
    return {
      'session_id': s.sessionId,
      'title': s.title,
      'cwd': s.cwd,
      'updated_at': s.updatedAt?.toIso8601String(),
      'input_tokens': s.inputTokens,
      'output_tokens': s.outputTokens,
      'total_tokens': s.totalTokens,
    };
  }

  static Future<String> _writeFile(
    String extension,
    String content, {
    Directory? directory,
  }) async {
    final dir = directory ?? await _exportDir();
    if (!await dir.exists()) {
      await dir.create(recursive: true);
    }
    final stamp = DateTime.now()
        .toIso8601String()
        .replaceAll(':', '-')
        .replaceAll('.', '-');
    final file = File('${dir.path}/codexus-sessions-$stamp.$extension');
    await file.writeAsString(content);
    return file.path;
  }

  static Future<Directory> _exportDir() async {
    final home = userHomeDir();
    if (home != null && home.trim().isNotEmpty) {
      final downloads = Directory('$home/Downloads');
      if (await downloads.exists()) {
        return downloads;
      }
      return Directory(home);
    }
    return Directory.systemTemp;
  }
}

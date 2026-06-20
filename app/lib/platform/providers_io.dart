import 'dart:convert';
import 'dart:io';

import 'user_paths.dart';

/// 供应商配置导出工具：把自定义供应商写成 JSON 到 `~/Downloads`。
///
/// 仅用 `dart:io`，导入侧走「粘贴 JSON」而非原生文件选择器，避免新增原生插件。
class ProvidersIo {
  static Future<String> exportToDownloads(
    List<Map<String, dynamic>> providers, {
    Directory? directory,
  }) async {
    const encoder = JsonEncoder.withIndent('  ');
    final content = encoder.convert(providers);
    final dir = directory ?? await _exportDir();
    if (!await dir.exists()) {
      await dir.create(recursive: true);
    }
    final stamp = DateTime.now()
        .toIso8601String()
        .replaceAll(':', '-')
        .replaceAll('.', '-');
    final file = File('${dir.path}/codexus-providers-$stamp.json');
    await file.writeAsString(content);
    return file.path;
  }

  /// 解析「粘贴的 JSON」为供应商列表，兼容裸数组或 `{ "providers": [...] }`。
  static List<Map<String, dynamic>> parseImport(String raw) {
    final decoded = jsonDecode(raw);
    final list = decoded is Map<String, dynamic>
        ? decoded['providers']
        : decoded;
    if (list is! List) {
      throw const FormatException('JSON 顶层应为供应商数组或 {"providers": [...]}');
    }
    return list.whereType<Map<String, dynamic>>().toList();
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

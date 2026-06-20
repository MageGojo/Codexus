import 'dart:convert';
import 'dart:io';

import 'package:app/platform/providers_io.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('parseImport 接受裸数组与 {providers:[...]} 两种形态', () {
    final fromArray = ProvidersIo.parseImport('[{"id":"a"},{"id":"b"}]');
    expect(fromArray.length, 2);
    expect(fromArray.first['id'], 'a');

    final fromObject = ProvidersIo.parseImport('{"providers":[{"id":"c"}]}');
    expect(fromObject.length, 1);
    expect(fromObject.first['id'], 'c');
  });

  test('parseImport 对非法 JSON 抛错', () {
    expect(() => ProvidersIo.parseImport('{"providers": 123}'),
        throwsA(isA<FormatException>()));
  });

  test('exportToDownloads 写出格式化 JSON', () async {
    final dir = await Directory.systemTemp.createTemp('codexferry_providers_io');
    addTearDown(() async {
      if (await dir.exists()) {
        await dir.delete(recursive: true);
      }
    });

    final path = await ProvidersIo.exportToDownloads(
      [
        {'id': 'x', 'name': 'X'},
      ],
      directory: dir,
    );
    final decoded = jsonDecode(await File(path).readAsString()) as List<dynamic>;
    expect(decoded.length, 1);
    expect((decoded.first as Map<String, dynamic>)['id'], 'x');
  });
}

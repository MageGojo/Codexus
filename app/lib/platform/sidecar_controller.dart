import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';

enum SidecarStatus { stopped, starting, running, missing, failed }

class SidecarController extends ChangeNotifier {
  Process? _process;
  SidecarStatus _status = SidecarStatus.stopped;
  String? _message;

  SidecarStatus get status => _status;
  String? get message => _message;
  bool get isRunning => _status == SidecarStatus.running;

  /// 管理 API 健康检查地址（与 daemon 默认 `FERRY_IPC_LISTEN` 一致）。
  static const String _healthUrl = 'http://127.0.0.1:15722/ipc/health';

  /// 轻量探测后端是否已在运行（避免重复拉起导致端口冲突「启动失败」）。
  Future<bool> _probeHealth({
    Duration timeout = const Duration(milliseconds: 800),
  }) async {
    final client = HttpClient()..connectionTimeout = timeout;
    try {
      final request = await client.getUrl(Uri.parse(_healthUrl)).timeout(timeout);
      final response = await request.close().timeout(timeout);
      final body =
          await response.transform(utf8.decoder).join().timeout(timeout);
      return response.statusCode == 200 && body.contains('"status"') &&
          body.contains('ok');
    } catch (_) {
      return false;
    } finally {
      client.close(force: true);
    }
  }

  Future<void> start() async {
    if (_process != null || _status == SidecarStatus.starting) {
      return;
    }
    // 已有健康的后端在跑（例如上次关窗后未退出的实例）：直接复用，
    // 避免再拉起一个会因端口被占而秒退的重复实例，被误判为「后端启动不了」。
    if (await _probeHealth()) {
      _setStatus(SidecarStatus.running, '已连接到运行中的后端');
      return;
    }
    final executable = _resolveExecutable();
    if (executable == null) {
      _setStatus(
        SidecarStatus.missing,
        '找不到 sidecar。已检查：${_candidateExecutables().join('、')}',
      );
      return;
    }

    _setStatus(SidecarStatus.starting, '正在启动 sidecar...');
    try {
      final process = await Process.start(executable, ['serve']);
      _process = process;
      _setStatus(SidecarStatus.running, 'sidecar 运行中，pid=${process.pid}');
      final stderrTail = <String>[];
      process.stdout.transform(utf8.decoder).listen((_) {});
      process.stderr.transform(utf8.decoder).listen((line) {
        final trimmed = line.trim();
        if (trimmed.isNotEmpty) {
          stderrTail.add(trimmed);
          if (stderrTail.length > 20) {
            stderrTail.removeAt(0);
          }
          _message = trimmed;
          notifyListeners();
        }
      });

      unawaited(
        process.exitCode.then((code) async {
          _process = null;
          if (code == 0) {
            if (_status == SidecarStatus.running) {
              _setStatus(SidecarStatus.stopped, 'sidecar 已退出，code=0');
            }
            return;
          }
          // 非零退出最常见的原因是端口被既有实例占用。若此刻后端仍健康，
          // 说明端口被另一实例持有，应视为「运行中」而非启动失败。
          if (await _probeHealth()) {
            _setStatus(
              SidecarStatus.running,
              '已连接到运行中的后端（端口已被既有实例占用）',
            );
          } else {
            final reason = stderrTail.isEmpty ? 'code=$code' : stderrTail.last;
            _setStatus(SidecarStatus.failed, 'sidecar 启动失败：$reason');
          }
        }),
      );
    } catch (error) {
      _process = null;
      _setStatus(SidecarStatus.failed, '启动 sidecar 失败：$error');
    }
  }

  Future<void> stop() async {
    final process = _process;
    if (process == null) {
      _setStatus(SidecarStatus.stopped, 'sidecar 未运行');
      return;
    }
    process.kill();
    _process = null;
    _setStatus(SidecarStatus.stopped, 'sidecar 已停止');
  }

  @override
  void dispose() {
    _process?.kill();
    super.dispose();
  }

  String? _resolveExecutable() {
    for (final candidate in _candidateExecutables()) {
      if (File(candidate).existsSync()) {
        return candidate;
      }
    }
    return null;
  }

  List<String> _candidateExecutables() {
    // 兼容旧环境变量名 CODEFERRY_SIDECAR（改名前用户可能已配置）。
    final override = Platform.environment['CODEXFERRY_SIDECAR'] ??
        Platform.environment['CODEFERRY_SIDECAR'];
    if (override != null && override.trim().isNotEmpty) {
      return [override.trim()];
    }

    // 守护进程可执行文件名：Windows 带 .exe 后缀。
    final bin = Platform.isWindows ? 'ferry-daemon.exe' : 'ferry-daemon';

    final candidates = <String>[];
    final seen = <String>{};
    void add(String path) {
      if (path.isNotEmpty && seen.add(path)) {
        candidates.add(path);
      }
    }

    final exeDir = File(Platform.resolvedExecutable).parent;

    // 1) 打包分发：
    //    - macOS：daemon 随 .app 置于 Contents/Resources/。
    //    - Windows / Linux：daemon 与 GUI 可执行文件同目录（Flutter 产物目录），
    //      也可能在其 data/ 子目录里。
    if (Platform.isMacOS) {
      final contents = exeDir.parent; // .../Contents/MacOS -> .../Contents
      add('${contents.path}/Resources/$bin');
    } else {
      add('${exeDir.path}/$bin');
      add('${exeDir.path}/data/$bin');
    }

    // 2) 开发期：从可执行文件所在目录逐级向上回溯，定位
    //    <project>/core/target/{debug,release}/<bin>。与当前工作目录无关，
    //    flutter run 与直接双击 build 产物两种情况都能命中。
    Directory dir = exeDir;
    for (var i = 0; i < 12; i++) {
      add('${dir.path}/core/target/debug/$bin');
      add('${dir.path}/core/target/release/$bin');
      // 开发文件夹名可能是 CodexFerry（已改名）或仍为旧的 CodeFerry，两者都试。
      add('${dir.path}/CodexFerry/core/target/debug/$bin');
      add('${dir.path}/CodexFerry/core/target/release/$bin');
      add('${dir.path}/CodeFerry/core/target/debug/$bin');
      add('${dir.path}/CodeFerry/core/target/release/$bin');
      final parent = dir.parent;
      if (parent.path == dir.path) {
        break; // 已到文件系统根。
      }
      dir = parent;
    }

    // 3) 兜底：相对当前工作目录（从 app/ 目录启动 flutter 时可用）。
    add('../core/target/debug/$bin');
    add('../core/target/release/$bin');
    add('../../core/target/debug/$bin');
    add('../../core/target/release/$bin');
    return candidates;
  }

  void _setStatus(SidecarStatus status, String message) {
    _status = status;
    _message = message;
    notifyListeners();
  }
}

import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;

import '../models/account_summary.dart';
import '../models/active_provider.dart';
import '../models/app_settings.dart';
import '../models/codex_status.dart';
import '../models/poem.dart';
import '../models/pool_snapshot.dart';
import '../models/provider_preset.dart';
import '../models/session_detail.dart';
import '../models/session_record.dart';
import '../models/stats.dart';
import '../models/weather.dart';
import '../platform/user_paths.dart';

class IpcClient {
  IpcClient({this.baseUrl = 'http://127.0.0.1:15722'});

  final String baseUrl;

  /// 管理 API 本地鉴权 token（daemon 启动时写入 `~/.codexferry/ipc_token`）。
  String? _token;

  /// 读取本地 token（成功后缓存；未找到则下次请求再试，因 daemon 可能稍后才启动）。
  Future<void> _ensureToken() async {
    if (_token != null && _token!.isNotEmpty) {
      return;
    }
    try {
      final home = userHomeDir();
      if (home == null || home.trim().isEmpty) {
        return;
      }
      final file = File('$home/.codexferry/ipc_token');
      if (await file.exists()) {
        final value = (await file.readAsString()).trim();
        if (value.isNotEmpty) {
          _token = value;
        }
      }
    } catch (_) {
      // 读取失败时静默；请求将以无 token 方式发出（health 仍可用）。
    }
  }

  Future<Map<String, String>> _headers([Map<String, String>? extra]) async {
    await _ensureToken();
    final headers = <String, String>{};
    if (extra != null) {
      headers.addAll(extra);
    }
    final token = _token;
    if (token != null && token.isNotEmpty) {
      headers['Authorization'] = 'Bearer $token';
    }
    return headers;
  }

  Future<bool> health() async {
    final json = await _getJson('/ipc/health');
    return json['status'] == 'ok';
  }

  Future<List<ProviderPreset>> providers() async {
    final json = await _getJson('/ipc/providers');
    return (json as List<dynamic>)
        .whereType<Map<String, dynamic>>()
        .map(ProviderPreset.fromJson)
        .toList();
  }

  Future<List<AccountSummary>> accounts() async {
    final json = await _getJson('/ipc/accounts');
    return (json as List<dynamic>)
        .whereType<Map<String, dynamic>>()
        .map(AccountSummary.fromJson)
        .toList();
  }

  Future<CodexStatus> codexStatus() async {
    final json = await _getJson('/ipc/codex/status');
    return CodexStatus.fromJson(json as Map<String, dynamic>);
  }

  Future<ActiveProvider> activeProvider() async {
    final json = await _getJson('/ipc/runtime/provider');
    return ActiveProvider.fromJson(json as Map<String, dynamic>);
  }

  /// 切换运行时供应商。可选 [accountId]：用该账号保存的 Key，并把该账号记入会话
  /// （按账号统计 token 用量）。可选 [apiKey]：显式覆盖并保存。
  Future<ActiveProvider> switchProvider(
    String providerId, {
    String? accountId,
    String? apiKey,
  }) async {
    final json = await _postJson('/ipc/runtime/provider', {
      'provider_id': providerId,
      if (accountId != null && accountId.isNotEmpty) 'account_id': accountId,
      if (apiKey != null && apiKey.isNotEmpty) 'api_key': apiKey,
    });
    return ActiveProvider.fromJson(json as Map<String, dynamic>);
  }

  Future<ProviderPreset> upsertProvider(Map<String, dynamic> body) async {
    final json = await _postJson('/ipc/providers', body);
    return ProviderPreset.fromJson(json as Map<String, dynamic>);
  }

  Future<void> deleteProvider(String id) async {
    await _deleteJson('/ipc/providers/${Uri.encodeComponent(id)}');
  }

  Future<void> setProviderApiKey(String id, String apiKey) async {
    await _postJson('/ipc/providers/${Uri.encodeComponent(id)}/api-key', {
      'api_key': apiKey,
    });
  }

  Future<List<Map<String, dynamic>>> exportProviders() async {
    final json = await _getJson('/ipc/providers/export');
    return (json as List<dynamic>).whereType<Map<String, dynamic>>().toList();
  }

  Future<int> importProviders(
    List<Map<String, dynamic>> providers, {
    bool replace = false,
  }) async {
    final json = await _postJson('/ipc/providers/import', {
      'providers': providers,
      'replace': replace,
    });
    return (json as Map<String, dynamic>)['imported'] as int? ?? 0;
  }

  /// 接管 Codex（写 `~/.codex/config.toml` 指向本地代理）。
  ///
  /// 第三方供应商默认 `requires_openai_auth=false`：Codex 不再要求 OpenAI 登录，
  /// 由后端注入占位 bearer，代理用自身存储的供应商 Key 转发。账号池(ChatGPT)
  /// 场景可传 [requiresOpenaiAuth]=true 走原生鉴权。
  Future<TakeoverResult> takeoverCodex({
    String model = 'gpt-5-codex',
    bool requiresOpenaiAuth = false,
  }) async {
    final json = await _postJson('/ipc/codex/takeover', {
      'provider_key': 'codexferry',
      'provider_name': 'Codexus',
      'base_url': 'http://127.0.0.1:15721/v1',
      'wire_api': 'responses',
      'requires_openai_auth': requiresOpenaiAuth,
      'model': model,
      'set_as_default': true,
    });
    return TakeoverResult.fromJson(json as Map<String, dynamic>);
  }

  /// 新增 API Key 账号。可选 [providerId] 把该 Key 归属到指定供应商
  /// （账号页选择的供应商），后端会同时把 Key 绑定给该供应商供代理取用；
  /// 省略或传 `codex` 表示通用账号。
  Future<AccountSummary> addApiKeyAccount(String apiKey, {String? providerId}) async {
    final body = <String, dynamic>{'api_key': apiKey};
    if (providerId != null && providerId.isNotEmpty && providerId != 'codex') {
      body['provider_id'] = providerId;
    }
    final json = await _postJson('/ipc/accounts/api-key', body);
    return AccountSummary.fromJson(json as Map<String, dynamic>);
  }

  Future<AccountSummary> loginWithChatGpt({bool openBrowser = true}) async {
    final json = await _postJson('/ipc/accounts/codex-login', {
      'open_browser': openBrowser,
    });
    return AccountSummary.fromJson(json as Map<String, dynamic>);
  }

  /// Token 三件套建号（粘贴 id_token / access_token /（可选）refresh_token）。
  Future<AccountSummary> addCodexTokenAccount({
    required String idToken,
    required String accessToken,
    String? refreshToken,
  }) async {
    final json = await _postJson('/ipc/accounts/codex-token', {
      'id_token': idToken,
      'access_token': accessToken,
      if (refreshToken != null && refreshToken.isNotEmpty)
        'refresh_token': refreshToken,
    });
    return AccountSummary.fromJson(json as Map<String, dynamic>);
  }

  /// 从本机 `~/.codex/auth.json` 导入账号。
  Future<AccountSummary> importCodexLocal() async {
    final json = await _postJson('/ipc/accounts/import-codex-local', {});
    return AccountSummary.fromJson(json as Map<String, dynamic>);
  }

  /// 粘贴 JSON 文本导入账号（由后端运行时自动识别格式：单对象 / 数组 /
  /// `{accounts:[...]}`，嵌套或扁平均可），返回成功导入的数量。
  Future<int> importCodexJson(String content) async {
    final json = await _postJson('/ipc/accounts/import-json', {
      'content': content,
    });
    return (json as Map<String, dynamic>)['imported'] as int? ?? 0;
  }

  Future<void> deleteAccount(String id) async {
    await _deleteJson('/ipc/accounts/$id');
  }

  /// 「使用」账号：后端按账号类型**自动识别路由**——OAuth(ChatGPT) 写
  /// `~/.codex/auth.json` 直连官方、撤销代理；API Key/中转/供应商保存 Key 并
  /// 接管 Codex 走本地代理。可选 [model] 覆盖该账号偏好模型。
  ///
  /// 返回 `mode`（`direct` 直连官方 / `proxy` 经本地代理）与最终接管的模型。
  Future<({String mode, String? model})> useAccount(
    String id, {
    String? model,
  }) async {
    final json = await _postJson('/ipc/accounts/${Uri.encodeComponent(id)}/use', {
      if (model != null && model.trim().isNotEmpty) 'model': model.trim(),
    });
    final map = json as Map<String, dynamic>;
    return (
      mode: map['mode'] as String? ?? 'proxy',
      model: map['model'] as String?,
    );
  }

  /// 编辑账号：自定义名称 / 标签 / 备注；API Key 账号可更新 Key。
  /// 仅传入需要修改的字段（label/note 传空串表示清除；tags 传入即整组替换）。
  Future<AccountSummary> updateAccount(
    String id, {
    String? label,
    List<String>? tags,
    String? note,
    String? apiKey,
    String? model,
  }) async {
    final json = await _postJson('/ipc/accounts/${Uri.encodeComponent(id)}/update', {
      'label': ?label,
      'tags': ?tags,
      'note': ?note,
      'model': ?model,
      if (apiKey != null && apiKey.isNotEmpty) 'api_key': apiKey,
    });
    return AccountSummary.fromJson(json as Map<String, dynamic>);
  }

  /// 自动获取某供应商可用模型（调上游 OpenAI 兼容 /models，失败回退内置目录）。
  /// 返回 (models, source: upstream|catalog, error?)。
  Future<({List<String> models, String source, String? error})> fetchProviderModels(
    String providerId,
  ) async {
    final json = await _postJson(
      '/ipc/providers/${Uri.encodeComponent(providerId)}/models',
      const {},
    );
    final map = json as Map<String, dynamic>;
    return (
      models: (map['models'] as List<dynamic>? ?? const [])
          .map((e) => e.toString())
          .toList(),
      source: map['source'] as String? ?? 'catalog',
      error: map['error'] as String?,
    );
  }

  Future<List<SessionRecord>> sessions({int limit = 50}) async {
    final json = await _getJson('/ipc/sessions?limit=$limit');
    return (json as List<dynamic>)
        .whereType<Map<String, dynamic>>()
        .map(SessionRecord.fromJson)
        .toList();
  }

  /// 单个会话详情（含聊天记录），来自本地 rollout。
  Future<SessionDetail> sessionDetail(String sessionId) async {
    final json = await _getJson(
      '/ipc/sessions/${Uri.encodeComponent(sessionId)}',
    );
    return SessionDetail.fromJson(json as Map<String, dynamic>);
  }

  /// 仪表盘统计（近 [days] 天的趋势序列 + 累计指标）。
  Future<Stats> stats({int days = 30}) async {
    final json = await _getJson('/ipc/stats?days=$days');
    return Stats.fromJson(json as Map<String, dynamic>);
  }

  // ---- 应用设置 ----

  Future<SettingsResponse> getSettings() async {
    final json = await _getJson('/ipc/settings');
    return SettingsResponse.fromJson(json as Map<String, dynamic>);
  }

  Future<SettingsResponse> saveSettings(AppSettings settings) async {
    final json = await _postJson('/ipc/settings', settings.toJson());
    return SettingsResponse.fromJson(json as Map<String, dynamic>);
  }

  /// 保存（或清除，传空字符串）apizero API Key。
  Future<void> setApizeroKey(String apiKey) async {
    await _postJson('/ipc/settings/apizero-key', {'api_key': apiKey});
  }

  // ---- 生活化集成：天气 / 古诗词 ----

  Future<WeatherInfo> weather({
    String? city,
    String type = 'realtime',
    bool refresh = false,
  }) async {
    final params = <String, String>{'type': type};
    if (city != null && city.trim().isNotEmpty) {
      params['city'] = city.trim();
    }
    if (refresh) {
      params['refresh'] = '1';
    }
    final query = Uri(queryParameters: params).query;
    final json = await _getJson('/ipc/integrations/weather?$query');
    return WeatherInfo.fromJson(json as Map<String, dynamic>);
  }

  Future<Poem> poem({String? type}) async {
    final json = await _postJson('/ipc/integrations/poem', {
      if (type != null && type.isNotEmpty) 'type': type,
    });
    return Poem.fromJson(json as Map<String, dynamic>);
  }

  // ---- 账号池 / 配额 / 路由模式 ----

  Future<PoolResponse> pool() async {
    final json = await _getJson('/ipc/runtime/pool');
    return PoolResponse.fromJson(json as Map<String, dynamic>);
  }

  Future<PoolResponse> setPoolStrategy(String strategy) async {
    final json = await _postJson('/ipc/runtime/pool/strategy', {
      'strategy': strategy,
    });
    return PoolResponse.fromJson(json as Map<String, dynamic>);
  }

  /// 主动探测并刷新各账号的 5h/7d 配额与健康。
  Future<PoolResponse> refreshPoolQuota() async {
    final json = await _postJson('/ipc/runtime/pool/refresh-quota', {});
    return PoolResponse.fromJson(json as Map<String, dynamic>);
  }

  Future<dynamic> _getJson(String path) async {
    final response = await http.get(
      Uri.parse('$baseUrl$path'),
      headers: await _headers(),
    );
    return _decode(response);
  }

  Future<dynamic> _postJson(String path, Map<String, dynamic> body) async {
    final response = await http.post(
      Uri.parse('$baseUrl$path'),
      headers: await _headers({'content-type': 'application/json'}),
      body: jsonEncode(body),
    );
    return _decode(response);
  }

  Future<dynamic> _deleteJson(String path) async {
    final response = await http.delete(
      Uri.parse('$baseUrl$path'),
      headers: await _headers(),
    );
    return _decode(response);
  }

  dynamic _decode(http.Response response) {
    final body = response.body.isEmpty ? '{}' : response.body;
    final json = jsonDecode(body);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      String? message;
      if (json is Map<String, dynamic>) {
        final error = json['error'];
        if (error is Map<String, dynamic>) {
          message = error['message']?.toString();
        }
      }
      throw IpcException(message ?? 'IPC 请求失败：${response.statusCode}');
    }
    return json;
  }
}

class IpcException implements Exception {
  const IpcException(this.message);

  final String message;

  @override
  String toString() => message;
}

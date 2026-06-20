class AccountSummary {
  const AccountSummary({
    required this.id,
    required this.provider,
    required this.displayName,
    required this.authMode,
    required this.storedInKeychain,
    this.email,
    this.accountId,
    this.lastRefresh,
    this.expiresAt,
    this.current = false,
    this.label,
    this.tags = const [],
    this.note,
    this.model,
    this.plan,
    this.tokensUsed = 0,
    this.inputTokens = 0,
    this.outputTokens = 0,
    this.requests = 0,
  });

  final String id;
  final String provider;
  final String displayName;
  final String authMode;
  final bool storedInKeychain;
  final String? email;
  final String? accountId;
  final DateTime? lastRefresh;
  final DateTime? expiresAt;

  /// 是否为本机 Codex 当前在用账号（后端按 `~/.codex/auth.json` 身份比对得出）。
  final bool current;

  /// 用户自定义名称（编辑弹窗回填用；展示名已由后端用 label 覆盖）。
  final String? label;

  /// 标签。
  final List<String> tags;

  /// 备注。
  final String? note;

  /// 账号偏好模型（中转/厂商账号选定的真实模型名）。
  final String? model;

  /// ChatGPT 订阅方案（plus/pro/team/enterprise/business 等，来自 id_token）。
  final String? plan;

  /// 该账号累计 token 用量（成功会话）。
  final int tokensUsed;
  final int inputTokens;
  final int outputTokens;

  /// 该账号累计请求数（成功会话）。
  final int requests;

  /// 是否为 ChatGPT(OAuth) 登录。
  bool get isOAuth => authMode == 'chatgpt';

  /// 归属到具体供应商的 API Key 账号（provider 非空且非通用 codex）。
  bool get vendorBound => provider.isNotEmpty && provider != 'codex';

  /// 是否可改 API Key（仅 API Key 账号）。
  bool get canEditKey => !isOAuth;

  factory AccountSummary.fromJson(Map<String, dynamic> json) {
    return AccountSummary(
      id: json['id'] as String? ?? '',
      provider: json['provider'] as String? ?? '',
      displayName: json['display_name'] as String? ?? '未知账号',
      authMode: json['auth_mode'] as String? ?? '',
      storedInKeychain: json['stored_in_keychain'] as bool? ?? false,
      email: json['email'] as String?,
      accountId: json['account_id'] as String?,
      lastRefresh: _parseDate(json['last_refresh']),
      expiresAt: _parseDate(json['expires_at']),
      current: json['current'] as bool? ?? false,
      label: json['label'] as String?,
      tags: (json['tags'] as List<dynamic>? ?? const [])
          .map((e) => e.toString())
          .toList(),
      note: json['note'] as String?,
      model: json['model'] as String?,
      plan: json['plan'] as String?,
      tokensUsed: json['tokens_used'] as int? ?? 0,
      inputTokens: json['input_tokens'] as int? ?? 0,
      outputTokens: json['output_tokens'] as int? ?? 0,
      requests: json['requests'] as int? ?? 0,
    );
  }
}

DateTime? _parseDate(Object? value) {
  if (value is! String || value.isEmpty) {
    return null;
  }
  return DateTime.tryParse(value);
}

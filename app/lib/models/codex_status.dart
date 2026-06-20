class CodexStatus {
  const CodexStatus({
    required this.home,
    required this.configPath,
    required this.exists,
    this.managed = false,
    this.mode = 'none',
    this.model,
    this.authAccountId,
  });

  final String home;
  final String configPath;
  final bool exists;

  /// config.toml 是否含码渡注入的供应商代理（`[model_providers.codexferry]`）。
  final bool managed;

  /// 当前 Codex 实际模式（以 ~/.codex 真实文件为准）：
  /// `proxy`(供应商代理) / `direct`(OAuth 账号直连官方) / `none`(未接管)。
  final String mode;

  /// config.toml 顶层当前生效模型。
  final String? model;

  /// 当前 `~/.codex/auth.json` 的 OAuth 账号 account_id（direct 模式）。
  final String? authAccountId;

  bool get isDirect => mode == 'direct';
  bool get isProxy => mode == 'proxy';
  bool get isManaged => mode != 'none';

  factory CodexStatus.fromJson(Map<String, dynamic> json) {
    return CodexStatus(
      home: json['home'] as String? ?? '',
      configPath: json['config_path'] as String? ?? '',
      exists: json['exists'] as bool? ?? false,
      managed: json['managed'] as bool? ?? false,
      mode: json['mode'] as String? ?? 'none',
      model: json['model'] as String?,
      authAccountId: json['auth_account_id'] as String?,
    );
  }
}

class TakeoverResult {
  const TakeoverResult({
    required this.configPath,
    required this.providerKey,
    this.backupPath,
  });

  final String configPath;
  final String providerKey;
  final String? backupPath;

  factory TakeoverResult.fromJson(Map<String, dynamic> json) {
    return TakeoverResult(
      configPath: json['config_path'] as String? ?? '',
      providerKey: json['provider_key'] as String? ?? '',
      backupPath: json['backup_path'] as String?,
    );
  }
}

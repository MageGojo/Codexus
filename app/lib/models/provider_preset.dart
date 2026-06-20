class ProviderPreset {
  const ProviderPreset({
    required this.id,
    required this.name,
    required this.baseUrl,
    required this.api,
    required this.defaultModel,
    required this.apiKeyEnv,
    required this.aliases,
    this.models = const [],
    this.kind = 'direct',
    this.builtin = false,
    this.customized = false,
  });

  final String id;
  final String name;
  final String baseUrl;
  final String api;
  final String defaultModel;
  final List<String> apiKeyEnv;
  final List<ModelAlias> aliases;

  /// 可选模型目录（供 Codex 端下拉选择真实模型名）。
  final List<String> models;

  /// 分组类型：`direct`(直连) / `relay`(中转)。
  final String kind;

  /// 是否内置预设（存在同 id 内置预设即 true，即使被自定义覆盖）。
  final bool builtin;

  /// 是否被用户自定义（纯自定义，或对内置预设做了覆盖）。
  final bool customized;

  bool get isRelay => kind == 'relay';

  /// 是否为内置预设的「覆盖」（可恢复默认）。
  bool get isOverride => builtin && customized;

  /// 是否为纯自定义供应商（可删除）。
  bool get isPureCustom => !builtin && customized;

  factory ProviderPreset.fromJson(Map<String, dynamic> json) {
    return ProviderPreset(
      id: json['id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      baseUrl: json['base_url'] as String? ?? '',
      api: json['api'] as String? ?? 'chat',
      defaultModel: json['default_model'] as String? ?? '',
      apiKeyEnv: (json['api_key_env'] as List<dynamic>? ?? const [])
          .map((item) => item.toString())
          .toList(),
      aliases: (json['aliases'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(ModelAlias.fromJson)
          .toList(),
      models: (json['models'] as List<dynamic>? ?? const [])
          .map((item) => item.toString())
          .toList(),
      kind: json['kind'] as String? ?? 'direct',
      builtin: json['builtin'] as bool? ?? false,
      customized: json['customized'] as bool? ?? false,
    );
  }
}

class ModelAlias {
  const ModelAlias({required this.from, required this.to});

  final String from;
  final String to;

  factory ModelAlias.fromJson(Map<String, dynamic> json) {
    return ModelAlias(
      from: json['from'] as String? ?? '',
      to: json['to'] as String? ?? '',
    );
  }
}

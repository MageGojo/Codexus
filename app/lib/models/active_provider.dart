class ActiveProvider {
  const ActiveProvider({
    required this.baseUrl,
    required this.apiType,
    required this.defaultModel,
    required this.apiKeyConfigured,
    this.providerId,
  });

  final String? providerId;
  final String baseUrl;
  final String apiType;
  final String defaultModel;
  final bool apiKeyConfigured;

  factory ActiveProvider.fromJson(Map<String, dynamic> json) {
    return ActiveProvider(
      providerId: json['provider_id'] as String?,
      baseUrl: json['base_url'] as String? ?? '',
      apiType: json['api_type'] as String? ?? 'chat',
      defaultModel: json['default_model'] as String? ?? '',
      apiKeyConfigured: json['api_key_configured'] as bool? ?? false,
    );
  }
}

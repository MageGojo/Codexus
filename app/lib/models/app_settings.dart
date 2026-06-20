/// 应用设置（对应后端 `ferry-config::AppSettings`）。
class AppSettings {
  const AppSettings({
    this.weatherCity = '',
    this.weatherAutoLocate = true,
    this.showWeather = true,
    this.showPoem = true,
    this.poemCategory = '',
    this.poolQuotaAware = true,
  });

  final String weatherCity;

  /// 是否自动定位当前城市（默认 true，优先于 weatherCity）。
  final bool weatherAutoLocate;
  final bool showWeather;
  final bool showPoem;
  final String poemCategory;
  final bool poolQuotaAware;

  AppSettings copyWith({
    String? weatherCity,
    bool? weatherAutoLocate,
    bool? showWeather,
    bool? showPoem,
    String? poemCategory,
    bool? poolQuotaAware,
  }) {
    return AppSettings(
      weatherCity: weatherCity ?? this.weatherCity,
      weatherAutoLocate: weatherAutoLocate ?? this.weatherAutoLocate,
      showWeather: showWeather ?? this.showWeather,
      showPoem: showPoem ?? this.showPoem,
      poemCategory: poemCategory ?? this.poemCategory,
      poolQuotaAware: poolQuotaAware ?? this.poolQuotaAware,
    );
  }

  Map<String, dynamic> toJson() => {
    'weather_city': weatherCity,
    'weather_auto_locate': weatherAutoLocate,
    'show_weather': showWeather,
    'show_poem': showPoem,
    'poem_category': poemCategory,
    'pool_quota_aware': poolQuotaAware,
  };

  factory AppSettings.fromJson(Map<String, dynamic> json) {
    return AppSettings(
      weatherCity: json['weather_city'] as String? ?? '',
      weatherAutoLocate: json['weather_auto_locate'] as bool? ?? true,
      showWeather: json['show_weather'] as bool? ?? true,
      showPoem: json['show_poem'] as bool? ?? true,
      poemCategory: json['poem_category'] as String? ?? '',
      poolQuotaAware: json['pool_quota_aware'] as bool? ?? true,
    );
  }
}

/// `/ipc/settings` 返回：设置 + apizero Key 是否已配置（不下发明文）。
class SettingsResponse {
  const SettingsResponse({
    required this.settings,
    required this.apizeroKeyConfigured,
  });

  final AppSettings settings;
  final bool apizeroKeyConfigured;

  factory SettingsResponse.fromJson(Map<String, dynamic> json) {
    return SettingsResponse(
      settings: AppSettings.fromJson(
        (json['settings'] as Map<String, dynamic>? ?? const {}),
      ),
      apizeroKeyConfigured: json['apizero_key_configured'] as bool? ?? false,
    );
  }
}

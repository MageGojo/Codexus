/// 天气摘要（对应后端 `/ipc/integrations/weather` 的归一化返回）。
class WeatherInfo {
  const WeatherInfo({
    required this.city,
    required this.skycon,
    required this.emoji,
    required this.skyconCode,
    this.temperature,
    this.apparentTemperature,
    this.humidityPercent,
    this.visibilityKm,
    required this.windText,
    required this.windLevelText,
    this.aqi,
    required this.aqiLevel,
    required this.aqiColor,
    this.pm25,
    this.forecastKeypoint,
    this.alerts = const [],
  });

  final String city;
  final String skycon;
  final String emoji;
  final String skyconCode;
  final double? temperature;
  final double? apparentTemperature;
  final double? humidityPercent;
  final double? visibilityKm;
  final String windText;
  final String windLevelText;
  final double? aqi;
  final String aqiLevel;
  final String aqiColor;
  final double? pm25;
  final String? forecastKeypoint;
  final List<WeatherAlert> alerts;

  factory WeatherInfo.fromJson(Map<String, dynamic> json) {
    double? d(Object? v) => v == null ? null : (v as num).toDouble();
    return WeatherInfo(
      city: json['city'] as String? ?? '',
      skycon: json['skycon'] as String? ?? '',
      emoji: json['emoji'] as String? ?? '',
      skyconCode: json['skycon_code'] as String? ?? '',
      temperature: d(json['temperature']),
      apparentTemperature: d(json['apparent_temperature']),
      humidityPercent: d(json['humidity_percent']),
      visibilityKm: d(json['visibility_km']),
      windText: json['wind_text'] as String? ?? '',
      windLevelText: json['wind_level_text'] as String? ?? '',
      aqi: d(json['aqi']),
      aqiLevel: json['aqi_level'] as String? ?? '',
      aqiColor: json['aqi_color'] as String? ?? '',
      pm25: d(json['pm25']),
      forecastKeypoint: json['forecast_keypoint'] as String?,
      alerts: (json['alerts'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(WeatherAlert.fromJson)
          .toList(),
    );
  }
}

class WeatherAlert {
  const WeatherAlert({
    required this.title,
    required this.color,
    required this.level,
  });

  final String title;
  final String color;
  final String level;

  factory WeatherAlert.fromJson(Map<String, dynamic> json) {
    return WeatherAlert(
      title: json['title'] as String? ?? '',
      color: json['color'] as String? ?? '',
      level: json['level'] as String? ?? '',
    );
  }
}

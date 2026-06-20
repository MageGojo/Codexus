import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/weather.dart';
import '../theme/app_theme.dart';
import 'glass_surface.dart';

/// 仪表盘天气卡：图标 + 温度 + 体感 + 风/湿度/能见度 + AQI + 预警 + 一句话总结。
/// 数据经后端 `/ipc/integrations/weather` 代理彩云天气，城市来自设置。
class WeatherCard extends StatefulWidget {
  const WeatherCard({super.key, required this.client, this.city});

  final IpcClient client;
  final String? city;

  @override
  State<WeatherCard> createState() => _WeatherCardState();
}

class _WeatherCardState extends State<WeatherCard> {
  late Future<WeatherInfo> _future;

  @override
  void initState() {
    super.initState();
    _future = widget.client.weather(city: widget.city);
  }

  @override
  void didUpdateWidget(covariant WeatherCard oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.city != widget.city) {
      _reload();
    }
  }

  /// [refresh] 为 true 时强制后端跳过缓存拉最新（手动刷新按钮用）；
  /// 默认 false（城市变化等自动加载走 1 小时缓存）。
  void _reload({bool refresh = false}) {
    setState(() {
      _future = widget.client.weather(city: widget.city, refresh: refresh);
    });
  }

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return GlassSurface(
      padding: const EdgeInsets.all(18),
      child: FutureBuilder<WeatherInfo>(
        future: _future,
        builder: (context, snap) {
          if (snap.connectionState == ConnectionState.waiting) {
            return const SizedBox(
              height: 96,
              child: Center(
                child: SizedBox.square(
                  dimension: 22,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              ),
            );
          }
          if (snap.hasError) {
            return _error(text, onSurface);
          }
          final w = snap.requireData;
          return _content(context, text, onSurface, w);
        },
      ),
    );
  }

  Widget _error(TextTheme text, Color onSurface) {
    return Row(
      children: [
        Icon(Icons.cloud_off, color: onSurface.withValues(alpha: 0.5)),
        const SizedBox(width: 12),
        Expanded(
          child: Text(
            '天气暂不可用（可在设置配置 apizero Key 提升额度）',
            style: text.bodySmall?.copyWith(
              color: onSurface.withValues(alpha: 0.6),
            ),
          ),
        ),
        IconButton(
          icon: const Icon(Icons.refresh, size: 18),
          tooltip: '重试',
          onPressed: () => _reload(refresh: true),
        ),
      ],
    );
  }

  Widget _content(
    BuildContext context,
    TextTheme text,
    Color onSurface,
    WeatherInfo w,
  ) {
    final temp = w.temperature == null
        ? '--'
        : '${w.temperature!.round()}°';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text(w.emoji.isEmpty ? '🌤️' : w.emoji,
                style: const TextStyle(fontSize: 38)),
            const SizedBox(width: 14),
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.baseline,
                  textBaseline: TextBaseline.alphabetic,
                  children: [
                    Text(
                      temp,
                      style: text.headlineMedium?.copyWith(
                        fontWeight: FontWeight.w800,
                        letterSpacing: -1,
                      ),
                    ),
                    const SizedBox(width: 8),
                    Text(
                      w.skycon,
                      style: text.titleSmall?.copyWith(
                        color: onSurface.withValues(alpha: 0.7),
                      ),
                    ),
                  ],
                ),
                Text(
                  '${w.city}${w.apparentTemperature != null ? ' · 体感 ${w.apparentTemperature!.round()}°' : ''}',
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.55),
                  ),
                ),
              ],
            ),
            const Spacer(),
            if (w.aqiLevel.isNotEmpty)
              _aqiPill(w),
            const SizedBox(width: 4),
            IconButton(
              icon: const Icon(Icons.refresh, size: 18),
              tooltip: '刷新天气',
              onPressed: () => _reload(refresh: true),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Wrap(
          spacing: 16,
          runSpacing: 6,
          children: [
            _metric(onSurface, Icons.air, '${w.windText} ${w.windLevelText}'),
            if (w.humidityPercent != null)
              _metric(onSurface, Icons.water_drop_outlined,
                  '湿度 ${w.humidityPercent!.round()}%'),
            if (w.visibilityKm != null)
              _metric(onSurface, Icons.visibility_outlined,
                  '能见度 ${w.visibilityKm!.round()}km'),
            if (w.pm25 != null)
              _metric(onSurface, Icons.blur_on, 'PM2.5 ${w.pm25!.round()}'),
          ],
        ),
        if (w.forecastKeypoint != null && w.forecastKeypoint!.isNotEmpty) ...[
          const SizedBox(height: 10),
          Row(
            children: [
              Icon(Icons.tips_and_updates_outlined,
                  size: 14, color: ferryAccent.withValues(alpha: 0.8)),
              const SizedBox(width: 6),
              Expanded(
                child: Text(
                  w.forecastKeypoint!,
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.65),
                  ),
                ),
              ),
            ],
          ),
        ],
        if (w.alerts.isNotEmpty) ...[
          const SizedBox(height: 8),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [for (final a in w.alerts) _alertChip(a)],
          ),
        ],
      ],
    );
  }

  Widget _metric(Color onSurface, IconData icon, String label) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 14, color: onSurface.withValues(alpha: 0.5)),
        const SizedBox(width: 5),
        Text(
          label,
          style: TextStyle(
            color: onSurface.withValues(alpha: 0.72),
            fontSize: 12.5,
          ),
        ),
      ],
    );
  }

  Widget _aqiPill(WeatherInfo w) {
    final Color c = _aqiColor(w.aqiColor);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: c.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: c.withValues(alpha: 0.45)),
      ),
      child: Text(
        'AQI ${w.aqi?.round() ?? '-'} ${w.aqiLevel}',
        style: TextStyle(
          color: c,
          fontSize: 11.5,
          fontWeight: FontWeight.w700,
        ),
      ),
    );
  }

  Widget _alertChip(WeatherAlert a) {
    final Color c = _alertColor(a.color);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: c.withValues(alpha: 0.15),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: c.withValues(alpha: 0.5)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.warning_amber_rounded, size: 13, color: c),
          const SizedBox(width: 4),
          Text(
            a.title,
            style: TextStyle(
              color: c,
              fontSize: 11,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }

  static Color _aqiColor(String name) {
    switch (name) {
      case 'green':
        return const Color(0xFF34D399);
      case 'yellow':
        return const Color(0xFFFBBF24);
      case 'orange':
        return const Color(0xFFFB923C);
      case 'red':
        return const Color(0xFFF87171);
      case 'purple':
        return const Color(0xFFA78BFA);
      case 'maroon':
        return const Color(0xFFB91C1C);
      default:
        return const Color(0xFF94A3B8);
    }
  }

  static Color _alertColor(String chinese) {
    if (chinese.contains('红')) return const Color(0xFFF87171);
    if (chinese.contains('橙')) return const Color(0xFFFB923C);
    if (chinese.contains('黄')) return const Color(0xFFFBBF24);
    if (chinese.contains('蓝')) return const Color(0xFF38BDF8);
    return const Color(0xFF94A3B8);
  }
}

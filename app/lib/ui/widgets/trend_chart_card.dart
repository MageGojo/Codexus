import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/material.dart';

import 'glass_surface.dart';

/// 一条曲线系列。
class ChartSeries {
  const ChartSeries({
    required this.name,
    required this.color,
    required this.values,
  });

  final String name;
  final Color color;
  final List<double> values;
}

/// 玻璃趋势曲线卡片：标题 + 图例 + `fl_chart` 平滑曲线（渐变区域、稀疏日期轴、
/// 触摸提示）。无数据时展示占位。
class TrendChartCard extends StatelessWidget {
  const TrendChartCard({
    super.key,
    required this.title,
    required this.subtitle,
    required this.labels,
    required this.series,
    this.height = 220,
  });

  final String title;
  final String subtitle;
  final List<String> labels;
  final List<ChartSeries> series;
  final double height;

  bool get _hasData =>
      series.any((s) => s.values.any((v) => v > 0)) && labels.isNotEmpty;

  double get _maxY {
    double m = 0;
    for (final s in series) {
      for (final v in s.values) {
        if (v > m) m = v;
      }
    }
    return m <= 0 ? 1 : m * 1.2;
  }

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;

    return GlassSurface(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      title,
                      style: text.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      subtitle,
                      style: text.bodySmall?.copyWith(
                        color: onSurface.withValues(alpha: 0.55),
                      ),
                    ),
                  ],
                ),
              ),
              _Legend(series: series, onSurface: onSurface),
            ],
          ),
          const SizedBox(height: 16),
          SizedBox(
            height: height,
            child: _hasData
                ? LineChart(_chartData(onSurface))
                : Center(
                    child: Text(
                      '暂无数据',
                      style: text.bodyMedium?.copyWith(
                        color: onSurface.withValues(alpha: 0.4),
                      ),
                    ),
                  ),
          ),
        ],
      ),
    );
  }

  LineChartData _chartData(Color onSurface) {
    final int n = labels.length;
    final double maxY = _maxY;
    final double bottomStep = (n / 5).ceilToDouble().clamp(1, n.toDouble());

    return LineChartData(
      minX: 0,
      maxX: (n - 1).toDouble(),
      minY: 0,
      maxY: maxY,
      gridData: FlGridData(
        show: true,
        drawVerticalLine: false,
        horizontalInterval: maxY / 4,
        getDrawingHorizontalLine: (value) => FlLine(
          color: onSurface.withValues(alpha: 0.08),
          strokeWidth: 1,
        ),
      ),
      borderData: FlBorderData(show: false),
      titlesData: FlTitlesData(
        topTitles: const AxisTitles(
          sideTitles: SideTitles(showTitles: false),
        ),
        rightTitles: const AxisTitles(
          sideTitles: SideTitles(showTitles: false),
        ),
        leftTitles: AxisTitles(
          sideTitles: SideTitles(
            showTitles: true,
            reservedSize: 42,
            interval: maxY / 4,
            getTitlesWidget: (value, meta) {
              if (value <= 0 && value != 0) return const SizedBox.shrink();
              return Padding(
                padding: const EdgeInsets.only(right: 6),
                child: Text(
                  _compact(value),
                  style: TextStyle(
                    color: onSurface.withValues(alpha: 0.45),
                    fontSize: 10,
                    fontFamily: 'monospace',
                  ),
                ),
              );
            },
          ),
        ),
        bottomTitles: AxisTitles(
          sideTitles: SideTitles(
            showTitles: true,
            reservedSize: 22,
            interval: bottomStep,
            getTitlesWidget: (value, meta) {
              final int i = value.round();
              if (i < 0 || i >= labels.length) {
                return const SizedBox.shrink();
              }
              return Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Text(
                  labels[i],
                  style: TextStyle(
                    color: onSurface.withValues(alpha: 0.45),
                    fontSize: 10,
                  ),
                ),
              );
            },
          ),
        ),
      ),
      lineTouchData: LineTouchData(
        touchTooltipData: LineTouchTooltipData(
          getTooltipColor: (_) => const Color(0xF21B2436),
          getTooltipItems: (spots) {
            return spots.map((s) {
              final int i = s.x.round();
              final String date = (i >= 0 && i < labels.length)
                  ? labels[i]
                  : '';
              final ChartSeries cs = series[s.barIndex];
              return LineTooltipItem(
                '$date\n${cs.name} ${_compact(s.y)}',
                TextStyle(
                  color: cs.color,
                  fontSize: 11.5,
                  fontWeight: FontWeight.w600,
                ),
              );
            }).toList();
          },
        ),
      ),
      lineBarsData: [
        for (final s in series)
          LineChartBarData(
            spots: [
              for (int i = 0; i < s.values.length; i++)
                FlSpot(i.toDouble(), s.values[i]),
            ],
            isCurved: true,
            curveSmoothness: 0.28,
            preventCurveOverShooting: true,
            color: s.color,
            barWidth: 2.5,
            isStrokeCapRound: true,
            dotData: const FlDotData(show: false),
            belowBarData: BarAreaData(
              show: series.length == 1,
              gradient: LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                colors: [
                  s.color.withValues(alpha: 0.28),
                  s.color.withValues(alpha: 0.02),
                ],
              ),
            ),
          ),
      ],
    );
  }

  static String _compact(double v) {
    if (v >= 1e9) return '${(v / 1e9).toStringAsFixed(1)}b';
    if (v >= 1e6) return '${(v / 1e6).toStringAsFixed(1)}m';
    if (v >= 1e3) return '${(v / 1e3).toStringAsFixed(1)}k';
    return v.toStringAsFixed(0);
  }
}

class _Legend extends StatelessWidget {
  const _Legend({required this.series, required this.onSurface});

  final List<ChartSeries> series;
  final Color onSurface;

  @override
  Widget build(BuildContext context) {
    return Wrap(
      spacing: 12,
      runSpacing: 4,
      children: [
        for (final s in series)
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 9,
                height: 9,
                decoration: BoxDecoration(
                  color: s.color,
                  borderRadius: BorderRadius.circular(3),
                ),
              ),
              const SizedBox(width: 5),
              Text(
                s.name,
                style: TextStyle(
                  color: onSurface.withValues(alpha: 0.7),
                  fontSize: 12,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ),
      ],
    );
  }
}

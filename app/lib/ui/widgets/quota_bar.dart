import 'package:flutter/material.dart';

import '../theme/app_theme.dart';

/// 配额窗口进度条（5h / 7d）：标签 + 已用百分比 + 渐变进度 + 重置倒计时。
class QuotaBar extends StatelessWidget {
  const QuotaBar({
    super.key,
    required this.label,
    required this.usedPercent,
    this.resetAt,
  });

  final String label;
  final double? usedPercent;
  final int? resetAt;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final double used = (usedPercent ?? 0).clamp(0, 100).toDouble();
    final double remaining = (100 - used).clamp(0, 100).toDouble();
    final Color barColor = _colorFor(used);
    final bool hasData = usedPercent != null;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text(
              label,
              style: TextStyle(
                color: onSurface.withValues(alpha: 0.7),
                fontSize: 11.5,
                fontWeight: FontWeight.w600,
              ),
            ),
            const Spacer(),
            Text(
              hasData ? '剩 ${remaining.toStringAsFixed(0)}%' : '—',
              style: TextStyle(
                color: hasData ? barColor : onSurface.withValues(alpha: 0.4),
                fontSize: 11.5,
                fontWeight: FontWeight.w700,
              ),
            ),
          ],
        ),
        const SizedBox(height: 5),
        ClipRRect(
          borderRadius: BorderRadius.circular(99),
          child: Stack(
            children: [
              Container(
                height: 7,
                color: onSurface.withValues(alpha: 0.10),
              ),
              FractionallySizedBox(
                widthFactor: hasData ? (used / 100).clamp(0.0, 1.0) : 0,
                child: Container(
                  height: 7,
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      colors: [barColor.withValues(alpha: 0.7), barColor],
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
        if (resetAt != null) ...[
          const SizedBox(height: 3),
          Text(
            '约 ${_resetIn(resetAt!)}后重置',
            style: TextStyle(
              color: onSurface.withValues(alpha: 0.42),
              fontSize: 10.5,
            ),
          ),
        ],
      ],
    );
  }

  static Color _colorFor(double used) {
    if (used >= 90) return const Color(0xFFF87171);
    if (used >= 70) return const Color(0xFFFBBF24);
    return const Color(0xFF34D399);
  }

  static String _resetIn(int epochSecs) {
    final reset = DateTime.fromMillisecondsSinceEpoch(epochSecs * 1000);
    final diff = reset.difference(DateTime.now());
    if (diff.isNegative) {
      return '0 分钟';
    }
    if (diff.inHours >= 24) {
      return '${diff.inDays} 天';
    }
    if (diff.inHours >= 1) {
      return '${diff.inHours} 小时';
    }
    return '${diff.inMinutes} 分钟';
  }
}

/// plan 徽章（free/plus/team/...），不同 plan 不同色。
class PlanBadge extends StatelessWidget {
  const PlanBadge({super.key, required this.plan});

  final String plan;

  @override
  Widget build(BuildContext context) {
    final Color color = _planColor(plan);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: color.withValues(alpha: 0.5)),
      ),
      child: Text(
        plan.toUpperCase(),
        style: TextStyle(
          color: color,
          fontSize: 10.5,
          fontWeight: FontWeight.w800,
          letterSpacing: 0.4,
        ),
      ),
    );
  }

  static Color _planColor(String plan) {
    switch (plan.toLowerCase()) {
      case 'enterprise':
        return const Color(0xFFA78BFA);
      case 'team':
      case 'business':
        return const Color(0xFF38BDF8);
      case 'pro':
        return const Color(0xFF22D3EE);
      case 'plus':
        return const Color(0xFF34D399);
      case 'free':
        return const Color(0xFF94A3B8);
      default:
        return const Color(0xFF94A3B8);
    }
  }
}

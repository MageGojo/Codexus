import 'dart:ui';

import 'package:flutter/material.dart';

import '../theme/app_theme.dart';

/// 极光背景:深/浅底色之上叠加几团强模糊的彩色光斑,为上层玻璃提供折射内容。
///
/// 刻意保持静态(无无限动画):既不拖累性能,也避免 `pumpAndSettle` 无法收敛;
/// 动效集中在 hover 与有限的入场过渡上。
class AuroraBackground extends StatelessWidget {
  const AuroraBackground({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final bool isDark = Theme.of(context).brightness == Brightness.dark;
    final Color base = isDark
        ? const Color(0xFF080B12)
        : const Color(0xFFEAF0F8);

    final List<_Blob> blobs = isDark
        ? const [
            _Blob(Alignment(-0.9, -1.05), 1.05, Color(0x8A38BDF8)),
            _Blob(Alignment(1.15, -0.7), 0.95, Color(0x735B7CFA)),
            _Blob(Alignment(-0.6, 1.2), 1.2, Color(0x5E18C3A6)),
            _Blob(Alignment(1.1, 1.05), 0.8, Color(0x4F8B5CF6)),
          ]
        : const [
            _Blob(Alignment(-0.9, -1.05), 1.05, Color(0x6638BDF8)),
            _Blob(Alignment(1.15, -0.7), 0.95, Color(0x4F5B7CFA)),
            _Blob(Alignment(-0.6, 1.2), 1.2, Color(0x3D18C3A6)),
            _Blob(Alignment(1.1, 1.05), 0.8, Color(0x338B5CF6)),
          ];

    return Stack(
      fit: StackFit.expand,
      children: [
        ColoredBox(color: base),
        Positioned.fill(
          child: IgnorePointer(
            child: ImageFiltered(
              imageFilter: ImageFilter.blur(sigmaX: 110, sigmaY: 110),
              child: LayoutBuilder(
                builder: (context, constraints) {
                  final double unit =
                      constraints.biggest.shortestSide.clamp(360, 1600);
                  return Stack(
                    children: [
                      for (final blob in blobs)
                        Align(
                          alignment: blob.alignment,
                          child: Container(
                            width: unit * blob.scale,
                            height: unit * blob.scale,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              gradient: RadialGradient(
                                colors: [blob.color, blob.color.withValues(alpha: 0)],
                              ),
                            ),
                          ),
                        ),
                    ],
                  );
                },
              ),
            ),
          ),
        ),
        // 顶部/边缘轻微压暗,提升前景文字对比。
        Positioned.fill(
          child: IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: RadialGradient(
                  center: const Alignment(0, -0.3),
                  radius: 1.25,
                  colors: [
                    Colors.transparent,
                    base.withValues(alpha: isDark ? 0.35 : 0.25),
                  ],
                ),
              ),
            ),
          ),
        ),
        child,
      ],
    );
  }
}

class _Blob {
  const _Blob(this.alignment, this.scale, this.color);

  final Alignment alignment;
  final double scale;
  final Color color;
}

/// 圆形强调色光晕,用于品牌图标等小处点缀(供其它组件复用)。
BoxDecoration accentGlowDecoration({double opacity = 0.3}) => BoxDecoration(
  shape: BoxShape.circle,
  gradient: RadialGradient(
    colors: [
      ferryAccent.withValues(alpha: opacity),
      ferryAccent.withValues(alpha: 0),
    ],
  ),
);

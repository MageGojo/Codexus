import 'dart:ui';

import 'package:flutter/material.dart';

import '../theme/app_theme.dart';

/// 液态玻璃面板:背景模糊 + 半透明渐变填充 + 1px 描边 + 顶部高光 + 带色外阴影。
///
/// 在「降低透明度」(高对比)无障碍场景下回退为纯色,保证可读性。
class GlassSurface extends StatelessWidget {
  const GlassSurface({
    super.key,
    required this.child,
    this.padding,
    this.margin,
    this.radius = FerryRadii.panel,
    this.strong = false,
    this.blur,
    this.onTap,
  });

  final Widget child;
  final EdgeInsetsGeometry? padding;
  final EdgeInsetsGeometry? margin;
  final double radius;
  final bool strong;
  final double? blur;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final GlassTokens glass = GlassTokens.of(context);
    final bool reduceTransparency = MediaQuery.maybeOf(context)?.highContrast ?? false;
    final BorderRadius br = BorderRadius.circular(radius);
    final List<Color> fillColors = strong ? glass.fillStrong : glass.fill;

    Widget surface = DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: br,
        color: reduceTransparency ? glass.scrim : null,
        gradient: reduceTransparency
            ? null
            : LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: fillColors,
              ),
        border: Border.all(color: glass.stroke, width: 1),
      ),
      child: padding == null ? child : Padding(padding: padding!, child: child),
    );

    Widget clipped = ClipRRect(
      borderRadius: br,
      child: reduceTransparency
          ? surface
          : BackdropFilter(
              filter: ImageFilter.blur(
                sigmaX: blur ?? glass.blurSigma,
                sigmaY: blur ?? glass.blurSigma,
              ),
              child: surface,
            ),
    );

    Widget stacked = Stack(
      children: [
        clipped,
        Positioned(
          left: radius,
          right: radius,
          top: 0,
          height: 1,
          child: IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  colors: [
                    Colors.transparent,
                    glass.highlight,
                    Colors.transparent,
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );

    Widget result = DecoratedBox(
      decoration: BoxDecoration(
        borderRadius: br,
        boxShadow: [
          BoxShadow(
            color: glass.shadow,
            blurRadius: 34,
            spreadRadius: -8,
            offset: const Offset(0, 20),
          ),
        ],
      ),
      child: stacked,
    );

    if (onTap != null) {
      result = _Pressable(radius: radius, onTap: onTap!, child: result);
    }
    if (margin != null) {
      result = Padding(padding: margin!, child: result);
    }
    return result;
  }
}

/// 圆形玻璃图标按钮(顶栏刷新、侧栏主题切换等)。
class GlassIconButton extends StatelessWidget {
  const GlassIconButton({
    super.key,
    required this.icon,
    required this.onPressed,
    this.tooltip,
    this.size = 40,
  });

  final IconData icon;
  final VoidCallback? onPressed;
  final String? tooltip;
  final double size;

  @override
  Widget build(BuildContext context) {
    final Color fg = Theme.of(context).colorScheme.onSurface;
    Widget button = GlassSurface(
      radius: size / 2,
      onTap: onPressed,
      child: SizedBox(
        width: size,
        height: size,
        child: Icon(icon, size: size * 0.46, color: fg.withValues(alpha: 0.9)),
      ),
    );
    if (tooltip != null) {
      button = Tooltip(message: tooltip!, child: button);
    }
    return button;
  }
}

class _Pressable extends StatefulWidget {
  const _Pressable({
    required this.child,
    required this.onTap,
    required this.radius,
  });

  final Widget child;
  final VoidCallback onTap;
  final double radius;

  @override
  State<_Pressable> createState() => _PressableState();
}

class _PressableState extends State<_Pressable> {
  bool _down = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: widget.onTap,
        onTapDown: (_) => setState(() => _down = true),
        onTapUp: (_) => setState(() => _down = false),
        onTapCancel: () => setState(() => _down = false),
        child: AnimatedScale(
          scale: _down ? 0.97 : 1,
          duration: const Duration(milliseconds: 120),
          curve: Curves.easeOut,
          child: widget.child,
        ),
      ),
    );
  }
}

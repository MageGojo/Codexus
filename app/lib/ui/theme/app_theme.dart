import 'dart:ui';

import 'package:flutter/material.dart';

/// 锁定的强调色：青蓝(sky),刻意避开「AI 紫」。整个 App 仅此一个强调色。
const Color ferryAccent = Color(0xFF38BDF8);

/// 下拉 / 弹出菜单的不透明背景色。
///
/// 主题把 `canvasColor` 设为透明（让内容浮在极光背景上），而 `DropdownButton`
/// 默认用 `canvasColor` 作菜单背景 → 菜单会变透明、文字糊在背景上。所有下拉都显式
/// 用本色作 `dropdownColor`，保证可读。
Color ferryMenuColor(BuildContext context) =>
    Theme.of(context).brightness == Brightness.dark
        ? const Color(0xFF1B2334)
        : const Color(0xFFFFFFFF);

/// 形状刻度(全局统一):面板/卡片/对话框 = 20,按钮/输入 = 12,小标签/弹出 = 10-14。
class FerryRadii {
  static const double panel = 20;
  static const double control = 12;
  static const double small = 10;
}

/// 液态玻璃材质令牌(Apple Liquid Glass 的 Flutter 近似实现,非 Apple 官方材质)。
///
/// 深浅两套;`GlassSurface` 等组件通过 [of] 读取,缺失时按亮度回退,
/// 保证在未注册扩展的测试环境也能正常渲染。
@immutable
class GlassTokens extends ThemeExtension<GlassTokens> {
  const GlassTokens({
    required this.blurSigma,
    required this.fill,
    required this.fillStrong,
    required this.stroke,
    required this.highlight,
    required this.shadow,
    required this.scrim,
  });

  /// 背景模糊强度。
  final double blurSigma;

  /// 普通面板的填充渐变(自上而下)。
  final List<Color> fill;

  /// 选中/强调面板的填充渐变。
  final List<Color> fillStrong;

  /// 1px 描边色。
  final Color stroke;

  /// 顶部高光线颜色(边缘折射感)。
  final Color highlight;

  /// 外阴影颜色(带色调,不用纯黑)。
  final Color shadow;

  /// 降低透明度无障碍场景的纯色回退。
  final Color scrim;

  static GlassTokens of(BuildContext context) =>
      Theme.of(context).extension<GlassTokens>() ??
      fallback(Theme.of(context).brightness);

  static GlassTokens fallback(Brightness brightness) =>
      brightness == Brightness.dark ? darkTokens : lightTokens;

  static const GlassTokens darkTokens = GlassTokens(
    blurSigma: 26,
    fill: [Color(0x21FFFFFF), Color(0x0AFFFFFF)],
    fillStrong: [Color(0x33FFFFFF), Color(0x14FFFFFF)],
    stroke: Color(0x26FFFFFF),
    highlight: Color(0x59FFFFFF),
    shadow: Color(0x803B4A6B),
    scrim: Color(0xF20E141F),
  );

  static const GlassTokens lightTokens = GlassTokens(
    blurSigma: 24,
    fill: [Color(0xB3FFFFFF), Color(0x73FFFFFF)],
    fillStrong: [Color(0xE6FFFFFF), Color(0xA6FFFFFF)],
    stroke: Color(0x99FFFFFF),
    highlight: Color(0xE6FFFFFF),
    shadow: Color(0x2638507A),
    scrim: Color(0xF7FFFFFF),
  );

  @override
  GlassTokens copyWith({
    double? blurSigma,
    List<Color>? fill,
    List<Color>? fillStrong,
    Color? stroke,
    Color? highlight,
    Color? shadow,
    Color? scrim,
  }) {
    return GlassTokens(
      blurSigma: blurSigma ?? this.blurSigma,
      fill: fill ?? this.fill,
      fillStrong: fillStrong ?? this.fillStrong,
      stroke: stroke ?? this.stroke,
      highlight: highlight ?? this.highlight,
      shadow: shadow ?? this.shadow,
      scrim: scrim ?? this.scrim,
    );
  }

  @override
  GlassTokens lerp(ThemeExtension<GlassTokens>? other, double t) {
    if (other is! GlassTokens) {
      return this;
    }
    List<Color> lerpList(List<Color> a, List<Color> b) => [
      Color.lerp(a[0], b[0], t) ?? a[0],
      Color.lerp(a[1], b[1], t) ?? a[1],
    ];
    return GlassTokens(
      blurSigma: lerpDouble(blurSigma, other.blurSigma, t) ?? blurSigma,
      fill: lerpList(fill, other.fill),
      fillStrong: lerpList(fillStrong, other.fillStrong),
      stroke: Color.lerp(stroke, other.stroke, t) ?? stroke,
      highlight: Color.lerp(highlight, other.highlight, t) ?? highlight,
      shadow: Color.lerp(shadow, other.shadow, t) ?? shadow,
      scrim: Color.lerp(scrim, other.scrim, t) ?? scrim,
    );
  }
}

/// 构建深/浅主题。Scaffold 透明,内容浮在极光背景之上;统一字体走 macOS 系统字体。
class FerryTheme {
  static ThemeData light() => _build(Brightness.light, GlassTokens.lightTokens);
  static ThemeData dark() => _build(Brightness.dark, GlassTokens.darkTokens);

  static ThemeData _build(Brightness brightness, GlassTokens glass) {
    final bool isDark = brightness == Brightness.dark;
    final ColorScheme scheme = ColorScheme.fromSeed(
      seedColor: ferryAccent,
      brightness: brightness,
    );
    final ThemeData base = ThemeData(
      brightness: brightness,
      useMaterial3: true,
      colorScheme: scheme,
    );
    final Color onSurface = scheme.onSurface;
    final Color dialogColor = isDark
        ? const Color(0xF21A2233)
        : const Color(0xFAFFFFFF);

    return base.copyWith(
      scaffoldBackgroundColor: Colors.transparent,
      canvasColor: Colors.transparent,
      extensions: <ThemeExtension<dynamic>>[glass],
      textTheme: base.textTheme.apply(
        fontFamily: '.AppleSystemUIFont',
        bodyColor: onSurface,
        displayColor: onSurface,
      ),
      primaryTextTheme: base.primaryTextTheme.apply(
        fontFamily: '.AppleSystemUIFont',
      ),
      iconTheme: IconThemeData(color: onSurface.withValues(alpha: 0.92)),
      dividerTheme: DividerThemeData(color: glass.stroke, thickness: 1),
      dialogTheme: DialogThemeData(
        backgroundColor: dialogColor,
        surfaceTintColor: Colors.transparent,
        elevation: 24,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(FerryRadii.panel),
        ),
      ),
      popupMenuTheme: PopupMenuThemeData(
        color: dialogColor,
        surfaceTintColor: Colors.transparent,
        elevation: 16,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(FerryRadii.small + 4),
        ),
      ),
      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        backgroundColor: isDark
            ? const Color(0xF2222B3D)
            : const Color(0xF21B2436),
        contentTextStyle: const TextStyle(color: Colors.white),
        actionTextColor: ferryAccent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(FerryRadii.small + 4),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        isDense: true,
        fillColor: isDark ? const Color(0x14FFFFFF) : const Color(0x0A1B2A4A),
        hintStyle: TextStyle(color: onSurface.withValues(alpha: 0.45)),
        labelStyle: TextStyle(color: onSurface.withValues(alpha: 0.7)),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(FerryRadii.control),
          borderSide: BorderSide(color: glass.stroke),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(FerryRadii.control),
          borderSide: BorderSide(color: glass.stroke),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(FerryRadii.control),
          borderSide: const BorderSide(color: ferryAccent, width: 1.5),
        ),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(FerryRadii.control),
          ),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          side: BorderSide(color: glass.stroke),
          foregroundColor: onSurface,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(FerryRadii.control),
          ),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(FerryRadii.small),
          ),
        ),
      ),
      chipTheme: base.chipTheme.copyWith(
        backgroundColor: isDark
            ? const Color(0x14FFFFFF)
            : const Color(0x0F1B2A4A),
        selectedColor: ferryAccent.withValues(alpha: isDark ? 0.30 : 0.20),
        side: BorderSide(color: glass.stroke),
        labelStyle: TextStyle(color: onSurface),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(FerryRadii.small),
        ),
      ),
    );
  }
}

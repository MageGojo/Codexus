import 'package:flutter/material.dart';
import 'package:window_manager/window_manager.dart';

import '../../platform/sidecar_controller.dart';
import '../theme/app_theme.dart';
import '../widgets/glass_surface.dart';

class SidebarItemData {
  const SidebarItemData({
    required this.icon,
    required this.selectedIcon,
    required this.label,
  });

  final IconData icon;
  final IconData selectedIcon;
  final String label;
}

/// 左侧玻璃导航栏:品牌、导航项、底部 sidecar 状态与主题切换。
class AppSidebar extends StatelessWidget {
  const AppSidebar({
    super.key,
    required this.items,
    required this.selectedIndex,
    required this.onSelect,
    required this.sidecar,
    this.themeMode,
    this.onCycleTheme,
  });

  final List<SidebarItemData> items;
  final int selectedIndex;
  final ValueChanged<int> onSelect;
  final SidecarController sidecar;
  final ThemeMode? themeMode;
  final VoidCallback? onCycleTheme;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 12, 0, 12),
      child: SizedBox(
        width: 234,
        child: GlassSurface(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // 顶部预留红绿灯位,并作为窗口拖拽区。
              DragToMoveArea(
                child: Padding(
                  padding: const EdgeInsets.only(top: 52, bottom: 6),
                  child: _Brand(),
                ),
              ),
              const SizedBox(height: 6),
              for (int i = 0; i < items.length; i++)
                _SidebarItem(
                  data: items[i],
                  selected: i == selectedIndex,
                  onTap: () => onSelect(i),
                ),
              const Spacer(),
              _SidecarStatus(status: sidecar.status),
              const SizedBox(height: 10),
              if (onCycleTheme != null) ...[
                _ThemeToggle(mode: themeMode ?? ThemeMode.system, onTap: onCycleTheme!),
                const SizedBox(height: 6),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _Brand extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Row(
      children: [
        Container(
          width: 40,
          height: 40,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(FerryRadii.control),
            gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [Color(0xFF38BDF8), Color(0xFF5B7CFA)],
            ),
            boxShadow: [
              BoxShadow(
                color: ferryAccent.withValues(alpha: 0.45),
                blurRadius: 16,
                spreadRadius: -4,
                offset: const Offset(0, 6),
              ),
            ],
          ),
          alignment: Alignment.center,
          child: const Text(
            '渡',
            style: TextStyle(
              color: Colors.white,
              fontWeight: FontWeight.w800,
              fontSize: 20,
            ),
          ),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                'Codexus',
                style: text.titleMedium?.copyWith(fontWeight: FontWeight.w700),
              ),
              Text(
                'Codexus 代理',
                style: text.bodySmall?.copyWith(
                  color: onSurface.withValues(alpha: 0.55),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _SidebarItem extends StatefulWidget {
  const _SidebarItem({
    required this.data,
    required this.selected,
    required this.onTap,
  });

  final SidebarItemData data;
  final bool selected;
  final VoidCallback onTap;

  @override
  State<_SidebarItem> createState() => _SidebarItemState();
}

class _SidebarItemState extends State<_SidebarItem> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final bool isDark = Theme.of(context).brightness == Brightness.dark;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final bool selected = widget.selected;

    final Color? bg = selected
        ? ferryAccent.withValues(alpha: isDark ? 0.18 : 0.16)
        : _hover
        ? onSurface.withValues(alpha: isDark ? 0.07 : 0.05)
        : null;
    final Color fg = selected
        ? (isDark ? const Color(0xFF8FD9FB) : const Color(0xFF0B6FA4))
        : onSurface.withValues(alpha: 0.82);

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        behavior: HitTestBehavior.opaque,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          curve: Curves.easeOut,
          margin: const EdgeInsets.symmetric(vertical: 3),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(FerryRadii.control),
            border: Border.all(
              color: selected
                  ? ferryAccent.withValues(alpha: 0.32)
                  : Colors.transparent,
            ),
          ),
          child: Row(
            children: [
              Icon(
                selected ? widget.data.selectedIcon : widget.data.icon,
                size: 20,
                color: selected ? ferryAccent : fg,
              ),
              const SizedBox(width: 12),
              Text(
                widget.data.label,
                style: TextStyle(
                  color: fg,
                  fontWeight: selected ? FontWeight.w600 : FontWeight.w500,
                  fontSize: 14,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _SidecarStatus extends StatelessWidget {
  const _SidecarStatus({required this.status});

  final SidecarStatus status;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final (Color dot, String label) = switch (status) {
      SidecarStatus.running => (const Color(0xFF34D399), '后端运行中'),
      SidecarStatus.starting => (const Color(0xFFFBBF24), '后端启动中'),
      SidecarStatus.failed => (const Color(0xFFF87171), '后端启动失败'),
      SidecarStatus.missing => (const Color(0xFFF87171), '后端未找到'),
      SidecarStatus.stopped => (onSurface.withValues(alpha: 0.4), '后端未运行'),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: onSurface.withValues(alpha: 0.05),
        borderRadius: BorderRadius.circular(FerryRadii.control),
        border: Border.all(color: onSurface.withValues(alpha: 0.08)),
      ),
      child: Row(
        children: [
          Container(
            width: 9,
            height: 9,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: dot,
              boxShadow: [
                BoxShadow(color: dot.withValues(alpha: 0.6), blurRadius: 8),
              ],
            ),
          ),
          const SizedBox(width: 10),
          Text(
            label,
            style: TextStyle(
              color: onSurface.withValues(alpha: 0.75),
              fontSize: 13,
            ),
          ),
        ],
      ),
    );
  }
}

class _ThemeToggle extends StatelessWidget {
  const _ThemeToggle({required this.mode, required this.onTap});

  final ThemeMode mode;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final (IconData icon, String label) = switch (mode) {
      ThemeMode.system => (Icons.brightness_auto_outlined, '跟随系统'),
      ThemeMode.light => (Icons.light_mode_outlined, '浅色'),
      ThemeMode.dark => (Icons.dark_mode_outlined, '深色'),
    };
    return GlassSurface(
      radius: FerryRadii.control,
      onTap: onTap,
      strong: true,
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 11),
      child: Row(
        children: [
          Icon(icon, size: 18, color: onSurface.withValues(alpha: 0.85)),
          const SizedBox(width: 10),
          Text(
            '主题 · $label',
            style: TextStyle(
              color: onSurface.withValues(alpha: 0.85),
              fontSize: 13,
              fontWeight: FontWeight.w500,
            ),
          ),
          const Spacer(),
          Icon(
            Icons.unfold_more,
            size: 16,
            color: onSurface.withValues(alpha: 0.4),
          ),
        ],
      ),
    );
  }
}

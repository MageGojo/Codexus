import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/poem.dart';
import '../theme/app_theme.dart';
import 'glass_surface.dart';

/// 古诗词点缀。仪表盘问候区用完整玻璃卡（含换一首），空态用轻量内联样式。
class PoemBanner extends StatefulWidget {
  const PoemBanner({
    super.key,
    required this.client,
    this.category,
    this.compact = false,
  });

  final IpcClient client;

  /// 诗词主题（空=随机）。
  final String? category;

  /// 紧凑模式：去掉玻璃卡背景，仅一行诗 + 出处，用于空态点缀。
  final bool compact;

  @override
  State<PoemBanner> createState() => _PoemBannerState();
}

class _PoemBannerState extends State<PoemBanner> {
  late Future<Poem> _future;

  @override
  void initState() {
    super.initState();
    _future = widget.client.poem(type: widget.category);
  }

  void _next() {
    setState(() {
      _future = widget.client.poem(type: widget.category);
    });
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<Poem>(
      future: _future,
      builder: (context, snap) {
        final poem = snap.data;
        if (snap.connectionState == ConnectionState.waiting && poem == null) {
          return widget.compact ? const SizedBox.shrink() : _shell(_loading());
        }
        if ((snap.hasError || poem == null || poem.isEmpty)) {
          return const SizedBox.shrink();
        }
        return widget.compact ? _compactBody(poem) : _shell(_fullBody(poem));
      },
    );
  }

  Widget _shell(Widget child) => GlassSurface(
    padding: const EdgeInsets.fromLTRB(18, 16, 12, 16),
    child: child,
  );

  Widget _loading() => const SizedBox(
    height: 30,
    child: Center(
      child: SizedBox.square(
        dimension: 18,
        child: CircularProgressIndicator(strokeWidth: 2),
      ),
    ),
  );

  Widget _fullBody(Poem poem) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Icon(Icons.format_quote_rounded,
            color: ferryAccent.withValues(alpha: 0.7), size: 22),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                poem.content,
                style: text.titleMedium?.copyWith(
                  fontWeight: FontWeight.w600,
                  height: 1.5,
                  letterSpacing: 0.3,
                ),
              ),
              if (poem.attribution.isNotEmpty) ...[
                const SizedBox(height: 6),
                Text(
                  '— ${poem.attribution}',
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.55),
                  ),
                ),
              ],
            ],
          ),
        ),
        IconButton(
          icon: const Icon(Icons.refresh, size: 18),
          tooltip: '换一首',
          onPressed: _next,
        ),
      ],
    );
  }

  Widget _compactBody(Poem poem) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          poem.content,
          textAlign: TextAlign.center,
          style: text.bodyMedium?.copyWith(
            color: onSurface.withValues(alpha: 0.6),
            height: 1.5,
            fontStyle: FontStyle.italic,
          ),
        ),
        if (poem.attribution.isNotEmpty) ...[
          const SizedBox(height: 4),
          Text(
            '— ${poem.attribution}',
            style: text.bodySmall?.copyWith(
              color: onSurface.withValues(alpha: 0.4),
            ),
          ),
        ],
      ],
    );
  }
}

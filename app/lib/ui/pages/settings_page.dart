import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/app_settings.dart';
import '../theme/app_theme.dart';
import '../widgets/empty_state.dart';
import '../widgets/glass_surface.dart';

/// 设置页：apizero Key、天气城市、生活化开关、古诗词主题、账号池调度策略。
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key, required this.client, this.onChanged});

  final IpcClient client;

  /// 设置保存后通知上层（用于刷新仪表盘天气/诗词）。
  final VoidCallback? onChanged;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  late Future<SettingsResponse> _future;
  final _cityCtrl = TextEditingController();
  final _keyCtrl = TextEditingController();

  AppSettings _settings = const AppSettings();
  bool _keyConfigured = false;
  bool _busy = false;
  bool _loaded = false;

  static const _poemTypes = <(String, String)>[
    ('', '随机'),
    ('shuqing', '抒情'),
    ('siji', '四季'),
    ('shanshui', '山水'),
    ('tianqi', '天气'),
    ('renwu', '人物'),
    ('shenghuo', '生活'),
    ('jieri', '节日'),
    ('dongwu', '动物'),
    ('zhiwu', '植物'),
    ('shiwu', '食物'),
  ];

  @override
  void initState() {
    super.initState();
    _future = _load();
  }

  Future<SettingsResponse> _load() async {
    final resp = await widget.client.getSettings();
    _settings = resp.settings;
    _keyConfigured = resp.apizeroKeyConfigured;
    _cityCtrl.text = _settings.weatherCity;
    _loaded = true;
    return resp;
  }

  @override
  void dispose() {
    _cityCtrl.dispose();
    _keyCtrl.dispose();
    super.dispose();
  }

  void _notify(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _save() async {
    setState(() => _busy = true);
    try {
      final next = _settings.copyWith(weatherCity: _cityCtrl.text.trim());
      final resp = await widget.client.saveSettings(next);
      _settings = resp.settings;
      // 同步账号池策略（与 quota-aware 开关一致）。
      await widget.client.setPoolStrategy(
        next.poolQuotaAware ? 'quota_aware' : 'round_robin',
      );
      _notify('设置已保存');
      widget.onChanged?.call();
    } catch (e) {
      _notify('保存失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _saveKey() async {
    final key = _keyCtrl.text.trim();
    setState(() => _busy = true);
    try {
      await widget.client.setApizeroKey(key);
      _keyCtrl.clear();
      setState(() => _keyConfigured = key.isNotEmpty);
      _notify(key.isEmpty ? '已清除 apizero Key' : 'apizero Key 已保存到本地凭据文件');
      widget.onChanged?.call();
    } catch (e) {
      _notify('保存 Key 失败：$e');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return FutureBuilder<SettingsResponse>(
      future: _future,
      builder: (context, snap) {
        if (!_loaded && snap.connectionState == ConnectionState.waiting) {
          return const Center(child: CircularProgressIndicator());
        }
        if (!_loaded && snap.hasError) {
          return EmptyState(
            icon: Icons.cloud_off,
            title: '设置不可用',
            message: '无法连接后端读取设置，请确认 ferry-daemon 在运行。',
          );
        }
        return ListView(
          padding: EdgeInsets.zero,
          children: [
            _apizeroSection(),
            const SizedBox(height: 16),
            _lifeSection(),
            const SizedBox(height: 16),
            _poolSection(),
            const SizedBox(height: 18),
            Align(
              alignment: Alignment.centerRight,
              child: FilledButton.icon(
                onPressed: _busy ? null : _save,
                icon: _busy
                    ? const SizedBox.square(
                        dimension: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.save_outlined),
                label: const Text('保存设置'),
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _section({required String title, required String subtitle, required List<Widget> children}) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return GlassSurface(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(title, style: text.titleMedium?.copyWith(fontWeight: FontWeight.w700)),
          const SizedBox(height: 2),
          Text(
            subtitle,
            style: text.bodySmall?.copyWith(color: onSurface.withValues(alpha: 0.55)),
          ),
          const SizedBox(height: 16),
          ...children,
        ],
      ),
    );
  }

  Widget _apizeroSection() {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return _section(
      title: 'apizero API Key',
      subtitle: '用于天气与古诗词接口，存到本地凭据文件（0600）。不配置也可匿名调用（额度较低）。',
      children: [
        Row(
          children: [
            Icon(
              _keyConfigured ? Icons.check_circle : Icons.info_outline,
              size: 16,
              color: _keyConfigured
                  ? const Color(0xFF34D399)
                  : onSurface.withValues(alpha: 0.5),
            ),
            const SizedBox(width: 6),
            Text(
              _keyConfigured ? 'Key 已配置' : '未配置（匿名额度）',
              style: TextStyle(
                color: onSurface.withValues(alpha: 0.7),
                fontSize: 12.5,
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _keyCtrl,
                obscureText: true,
                decoration: InputDecoration(
                  hintText: _keyConfigured ? '输入新 Key 以更新，或留空保存以清除' : '粘贴 apizero API Key',
                  prefixIcon: const Icon(Icons.vpn_key_outlined, size: 18),
                ),
              ),
            ),
            const SizedBox(width: 10),
            FilledButton(
              onPressed: _busy ? null : _saveKey,
              child: const Text('保存'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        SelectableText(
          '申请地址：https://apizero.cn/account/keys',
          style: TextStyle(
            color: onSurface.withValues(alpha: 0.45),
            fontSize: 11.5,
          ),
        ),
      ],
    );
  }

  Widget _lifeSection() {
    final bool auto = _settings.weatherAutoLocate;
    return _section(
      title: '生活化点缀',
      subtitle: '在仪表盘展示天气与古诗词，按需可关闭以省额度。',
      children: [
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('自动定位当前城市'),
          subtitle: const Text('按你的网络位置自动取天气（默认开启）'),
          value: auto,
          activeThumbColor: ferryAccent,
          onChanged: (v) =>
              setState(() => _settings = _settings.copyWith(weatherAutoLocate: v)),
        ),
        const SizedBox(height: 8),
        TextField(
          controller: _cityCtrl,
          enabled: !auto,
          decoration: InputDecoration(
            labelText: '手动城市',
            hintText: auto ? '已开启自动定位（关闭后此处生效）' : '如 上海 / 深圳 / 朝阳区',
            prefixIcon: const Icon(Icons.location_on_outlined, size: 18),
          ),
        ),
        const SizedBox(height: 6),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('显示天气卡'),
          value: _settings.showWeather,
          activeThumbColor: ferryAccent,
          onChanged: (v) => setState(() => _settings = _settings.copyWith(showWeather: v)),
        ),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('显示古诗词'),
          value: _settings.showPoem,
          activeThumbColor: ferryAccent,
          onChanged: (v) => setState(() => _settings = _settings.copyWith(showPoem: v)),
        ),
        const SizedBox(height: 6),
        InputDecorator(
          decoration: const InputDecoration(
            labelText: '古诗词主题',
            prefixIcon: Icon(Icons.menu_book_outlined, size: 18),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              isDense: true,
              isExpanded: true,
              value: _settings.poemCategory,
              dropdownColor: ferryMenuColor(context),
              borderRadius: BorderRadius.circular(FerryRadii.control),
              items: [
                for (final (value, label) in _poemTypes)
                  DropdownMenuItem(value: value, child: Text(label)),
              ],
              onChanged: (v) => setState(
                () => _settings = _settings.copyWith(poemCategory: v ?? ''),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _poolSection() {
    return _section(
      title: '账号池调度',
      subtitle: '配额感知：轮询时优先选剩余额度多、plan 更高的账号（需 pool 模式）。',
      children: [
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('启用配额感知调度'),
          subtitle: const Text('关闭则使用普通轮询 (round-robin)'),
          value: _settings.poolQuotaAware,
          activeThumbColor: ferryAccent,
          onChanged: (v) =>
              setState(() => _settings = _settings.copyWith(poolQuotaAware: v)),
        ),
      ],
    );
  }
}

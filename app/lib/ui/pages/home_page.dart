import 'dart:async';

import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/account_summary.dart';
import '../../models/active_provider.dart';
import '../../models/codex_status.dart';
import '../../models/pool_snapshot.dart';
import '../../models/provider_preset.dart';
import '../../models/session_detail.dart';
import '../../models/session_record.dart';
import '../../models/stats.dart';
import '../../models/app_settings.dart';
import '../../platform/providers_io.dart';
import '../../platform/session_exporter.dart';
import '../../platform/sidecar_controller.dart';
import '../dialogs/add_account_dialog.dart';
import '../dialogs/edit_account_dialog.dart';
import '../shell/app_sidebar.dart';
import '../theme/app_theme.dart';
import '../widgets/account_card.dart';
import '../widgets/aurora_background.dart';
import '../widgets/empty_state.dart';
import '../widgets/glass_surface.dart';
import '../widgets/metric_card.dart';
import '../widgets/poem_banner.dart';
import '../widgets/trend_chart_card.dart';
import '../widgets/weather_card.dart';
import 'settings_page.dart';

class HomePage extends StatefulWidget {
  const HomePage({
    super.key,
    required this.client,
    required this.sidecar,
    this.themeMode,
    this.onCycleTheme,
  });

  final IpcClient client;
  final SidecarController sidecar;
  final ThemeMode? themeMode;
  final VoidCallback? onCycleTheme;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  int _selectedIndex = 0;
  AppSnapshot? _data;
  bool _loading = false;
  bool _starting = false;

  /// 每分钟自动刷新一次全部数据（仪表盘 Token 总量/今日用量、账号、会话等保持实时，
  /// 用户无需手点刷新）。仅在后端已连接时触发，避免离线时反复转圈。
  Timer? _autoRefresh;
  static const _autoRefreshInterval = Duration(minutes: 1);

  IpcClient get _client => widget.client;
  SidecarController get _sidecar => widget.sidecar;

  static const _navItems = [
    SidebarItemData(
      icon: Icons.dashboard_outlined,
      selectedIcon: Icons.dashboard,
      label: '仪表盘',
    ),
    SidebarItemData(
      icon: Icons.hub_outlined,
      selectedIcon: Icons.hub,
      label: '供应商',
    ),
    SidebarItemData(
      icon: Icons.key_outlined,
      selectedIcon: Icons.key,
      label: '账号',
    ),
    SidebarItemData(
      icon: Icons.history_outlined,
      selectedIcon: Icons.history,
      label: '会话',
    ),
    SidebarItemData(
      icon: Icons.settings_outlined,
      selectedIcon: Icons.settings,
      label: '设置',
    ),
  ];

  static const _subtitles = [
    '后端、供应商、账号与会话总览',
    '内置预设与自定义供应商,一键接管 Codex',
    'API Key 与 ChatGPT 账号管理',
    '经Codexus代理的请求记录与导出',
    'apizero Key、天气城市、生活化点缀与账号池调度',
  ];

  @override
  void initState() {
    super.initState();
    _sidecar.addListener(_onSidecarChanged);
    _bootstrap();
    _autoRefresh = Timer.periodic(_autoRefreshInterval, (_) {
      // 仅在已有数据且后端在线时静默刷新；离线/启动中由 bootstrap 流程负责。
      if (!mounted || _loading || _starting) {
        return;
      }
      if (_data?.ipcOnline ?? false) {
        _refresh();
      }
    });
  }

  @override
  void dispose() {
    _autoRefresh?.cancel();
    _sidecar.removeListener(_onSidecarChanged);
    super.dispose();
  }

  void _onSidecarChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  Future<AppSnapshot> _load() async {
    final results = await Future.wait<dynamic>([
      _client.health(),
      _client.providers(),
      _client.accounts(),
      _client.sessions(limit: 50),
      _client.codexStatus(),
      _client.activeProvider(),
      _client.stats(),
    ]);
    return AppSnapshot(
      ipcOnline: results[0] as bool,
      providers: results[1] as List<ProviderPreset>,
      accounts: results[2] as List<AccountSummary>,
      sessions: results[3] as List<SessionRecord>,
      codexStatus: results[4] as CodexStatus,
      activeProvider: results[5] as ActiveProvider,
      stats: results[6] as Stats,
    );
  }

  /// 刷新全部数据：保留当前已显示的旧数据，后台静默拉取，完成后整体替换
  /// （切页 / 进入页面 / 手动刷新都走它，体验上不闪烁、不需手点刷新）。
  Future<void> _refresh() async {
    if (_loading) {
      return;
    }
    setState(() => _loading = true);
    try {
      final snap = await _load();
      if (!mounted) return;
      setState(() {
        _data = snap;
        _loading = false;
      });
    } catch (error) {
      if (!mounted) return;
      // 静默刷新失败：保留已显示的旧数据，仅记录日志（未连接时由 _data==null 分支兜底引导）。
      debugPrint('Codexus刷新数据失败: $error');
      setState(() => _loading = false);
    }
  }

  void _reload() => _refresh();

  /// 打开软件即自动：探测后端 → 未连接则自动拉起 sidecar → 全量刷新数据。
  /// （无需用户手点「启动后端」或「刷新」；切页也会自动刷新。）
  Future<void> _bootstrap() async {
    if (await _pingHealth()) {
      await _refresh();
    } else {
      await _startSidecarThenReload();
    }
  }

  /// 探测后端健康，失败返回 false（不抛异常，供自启动判定用）。
  Future<bool> _pingHealth() async {
    try {
      return await _client.health();
    } catch (_) {
      return false;
    }
  }

  /// 拉起 sidecar，轮询等待后端就绪后全量刷新（自启动与手动「启动后端」共用）。
  Future<void> _startSidecarThenReload() async {
    if (mounted) {
      setState(() => _starting = true);
    }
    await _sidecar.start();
    // 轮询等待后端就绪（最多约 6s），就绪即继续，不固定死等。
    for (var i = 0; i < 20; i++) {
      await Future.delayed(const Duration(milliseconds: 300));
      if (await _pingHealth()) {
        break;
      }
    }
    if (!mounted) {
      return;
    }
    setState(() => _starting = false);
    await _refresh();
  }

  Future<void> _takeoverCodex(ProviderPreset provider) async {
    final active = await _client.switchProvider(provider.id);
    final result = await _client.takeoverCodex();
    if (!mounted) {
      return;
    }
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          '已切换到 ${provider.name}（${active.defaultModel}），并写入 Codex 接管配置：${result.providerKey}。',
        ),
      ),
    );
    _reload();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.transparent,
      body: AuroraBackground(
        child: SafeArea(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              AppSidebar(
                items: _navItems,
                selectedIndex: _selectedIndex,
                onSelect: (index) {
                  setState(() => _selectedIndex = index);
                  _refresh();
                },
                sidecar: _sidecar,
                themeMode: widget.themeMode,
                onCycleTheme: widget.onCycleTheme,
              ),
              Expanded(
                child: Builder(
                  builder: (context) {
                    if (_data == null && (_loading || _starting)) {
                      return Center(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            const CircularProgressIndicator(),
                            const SizedBox(height: 16),
                            Text(_starting ? '正在启动后端…' : '正在加载…'),
                          ],
                        ),
                      );
                    }
                    if (_data == null) {
                      return _PageShell(
                        title: _navItems[_selectedIndex].label,
                        subtitle: _subtitles[_selectedIndex],
                        onRefresh: _reload,
                        child: EmptyState(
                          icon: Icons.cloud_off,
                          title: '后端未连接',
                          message:
                              _sidecar.message ??
                              'Codexus需要本地 ferry-daemon（默认 127.0.0.1:15722）。点「启动后端」拉起，或确认它已在运行。',
                          action: Wrap(
                            spacing: 12,
                            runSpacing: 8,
                            alignment: WrapAlignment.center,
                            children: [
                              FilledButton.icon(
                                onPressed: _starting
                                    ? null
                                    : _startSidecarThenReload,
                                icon: _starting
                                    ? const SizedBox.square(
                                        dimension: 16,
                                        child: CircularProgressIndicator(
                                          strokeWidth: 2,
                                        ),
                                      )
                                    : const Icon(Icons.play_arrow),
                                label: Text(_starting ? '启动中...' : '启动后端 (Sidecar)'),
                              ),
                              OutlinedButton.icon(
                                onPressed: _reload,
                                icon: const Icon(Icons.refresh),
                                label: const Text('重试'),
                              ),
                            ],
                          ),
                        ),
                      );
                    }
                    final data = _data!;
                    return _PageShell(
                      title: _navItems[_selectedIndex].label,
                      subtitle: _subtitles[_selectedIndex],
                      onRefresh: _reload,
                      child: KeyedSubtree(
                        key: ValueKey(_selectedIndex),
                        child: switch (_selectedIndex) {
                          0 => _Dashboard(
                            data: data,
                            sidecar: _sidecar,
                            client: _client,
                          ),
                          1 => _ProvidersPage(
                            providers: data.providers,
                            codexStatus: data.codexStatus,
                            activeProvider: data.activeProvider,
                            client: _client,
                            onTakeover: _takeoverCodex,
                            onChanged: _reload,
                          ),
                          2 => _AccountsPage(
                            accounts: data.accounts,
                            providers: data.providers,
                            stats: data.stats,
                            client: _client,
                            onChanged: _reload,
                          ),
                          3 => _SessionsPage(
                            sessions: data.sessions,
                            client: _client,
                          ),
                          _ => SettingsPage(client: _client, onChanged: _reload),
                        },
                      ),
                    );
                  },
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class AppSnapshot {
  const AppSnapshot({
    required this.ipcOnline,
    required this.providers,
    required this.accounts,
    required this.sessions,
    required this.codexStatus,
    required this.activeProvider,
    required this.stats,
  });

  final bool ipcOnline;
  final List<ProviderPreset> providers;
  final List<AccountSummary> accounts;
  final List<SessionRecord> sessions;
  final CodexStatus codexStatus;
  final ActiveProvider activeProvider;
  final Stats stats;
}

class _PageShell extends StatelessWidget {
  const _PageShell({
    required this.title,
    required this.subtitle,
    required this.child,
    required this.onRefresh,
  });

  final String title;
  final String subtitle;
  final Widget child;
  final VoidCallback onRefresh;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 18, 24, 22),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      style: text.headlineMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                        letterSpacing: -0.5,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      subtitle,
                      style: text.bodyMedium?.copyWith(
                        color: onSurface.withValues(alpha: 0.55),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              GlassIconButton(
                icon: Icons.refresh,
                tooltip: '刷新',
                onPressed: onRefresh,
              ),
            ],
          ),
          const SizedBox(height: 18),
          Expanded(
            child: AnimatedSwitcher(
              duration: const Duration(milliseconds: 260),
              switchInCurve: Curves.easeOutCubic,
              transitionBuilder: (child, animation) {
                return FadeTransition(
                  opacity: animation,
                  child: SlideTransition(
                    position: Tween<Offset>(
                      begin: const Offset(0, 0.02),
                      end: Offset.zero,
                    ).animate(animation),
                    child: child,
                  ),
                );
              },
              child: child,
            ),
          ),
        ],
      ),
    );
  }
}

String _sidecarLabel(SidecarStatus status) {
  return switch (status) {
    SidecarStatus.stopped => '未运行',
    SidecarStatus.starting => '启动中',
    SidecarStatus.running => '运行中',
    SidecarStatus.missing => '未找到',
    SidecarStatus.failed => '启动失败',
  };
}

class _Dashboard extends StatefulWidget {
  const _Dashboard({
    required this.data,
    required this.sidecar,
    required this.client,
  });

  final AppSnapshot data;
  final SidecarController sidecar;
  final IpcClient client;

  @override
  State<_Dashboard> createState() => _DashboardState();
}

class _DashboardState extends State<_Dashboard> {
  AppSettings? _settings;

  AppSnapshot get data => widget.data;
  SidecarController get sidecar => widget.sidecar;

  @override
  void initState() {
    super.initState();
    _loadSettings();
  }

  Future<void> _loadSettings() async {
    try {
      final resp = await widget.client.getSettings();
      if (mounted) {
        setState(() => _settings = resp.settings);
      }
    } catch (_) {
      // 设置/后端不可用时静默：不展示天气与诗词，仪表盘其余照常。
    }
  }

  @override
  Widget build(BuildContext context) {
    final StatsTotals t = data.stats.totals;
    final int accountsCurrent = t.accountsCurrent > 0
        ? t.accountsCurrent
        : data.accounts.length;
    final CodexStatus cs = data.codexStatus;
    AccountSummary? currentAccount;
    for (final a in data.accounts) {
      if (a.current) {
        currentAccount = a;
        break;
      }
    }

    final metrics = <Widget>[
      MetricCard(
        title: '后端状态',
        value: data.ipcOnline ? '在线' : '离线',
        subtitle: '127.0.0.1:15722',
        icon: Icons.dns_outlined,
        accent: data.ipcOnline
            ? const Color(0xFF34D399)
            : const Color(0xFFF87171),
      ),
      MetricCard(
        title: '累计 Token',
        value: _compactNum(t.totalTokens),
        subtitle: '输入 ${_compactNum(t.inputTokens)} · 输出 ${_compactNum(t.outputTokens)}',
        icon: Icons.token_outlined,
      ),
      MetricCard(
        title: '今日 Token',
        value: _compactNum(
          data.stats.series.isNotEmpty ? data.stats.series.last.tokens : 0,
        ),
        subtitle: data.stats.series.isNotEmpty
            ? '输入 ${_compactNum(data.stats.series.last.inputTokens)} · 输出 ${_compactNum(data.stats.series.last.outputTokens)}'
            : '今日暂无对话',
        icon: Icons.today_outlined,
      ),
      MetricCard(
        title: '累计请求',
        value: _compactNum(t.requests),
        subtitle: '成功 ${t.succeeded} · 失败 ${t.failed}',
        icon: Icons.swap_horiz,
      ),
      MetricCard(
        title: '成功率',
        value: '${(t.successRate * 100).toStringAsFixed(t.requests > 0 ? 1 : 0)}%',
        subtitle: '基于全部请求',
        icon: Icons.check_circle_outline,
        accent: t.successRate >= 0.9
            ? const Color(0xFF34D399)
            : (t.successRate >= 0.6 ? const Color(0xFFFBBF24) : ferryAccent),
      ),
      MetricCard(
        title: '当前账号',
        value: '$accountsCurrent',
        subtitle: '新增 ${t.accountsAdded} · 删除 ${t.accountsDeleted}',
        icon: Icons.key_outlined,
      ),
      MetricCard(
        title: '账号存活率',
        value: '${(t.survivalRate * 100).toStringAsFixed(0)}%',
        subtitle: '失效(过期) ${t.accountsExpired}',
        icon: Icons.favorite_outline,
        accent: t.survivalRate >= 0.8
            ? const Color(0xFF34D399)
            : (t.survivalRate >= 0.5 ? const Color(0xFFFBBF24) : const Color(0xFFF87171)),
      ),
      MetricCard(
        title: 'Codex 状态',
        value: switch (cs.mode) {
          'proxy' => '代理接管',
          'direct' => '账号直连',
          _ => '未接管',
        },
        subtitle: switch (cs.mode) {
          'proxy' => '经Codexus本地代理转换',
          'direct' => currentAccount != null
              ? '直连 · ${currentAccount.displayName}'
              : '账号直连官方',
          _ => '尚未接管',
        },
        icon: Icons.settings_outlined,
        accent: cs.mode == 'none' ? ferryAccent : const Color(0xFF34D399),
      ),
      MetricCard(
        title: '当前模型',
        value: (cs.model ?? '').isNotEmpty
            ? cs.model!
            : (cs.mode == 'direct' ? 'gpt-5-codex' : '未设置'),
        subtitle: switch (cs.mode) {
          'direct' => 'ChatGPT 直连官方',
          'proxy' => '经Codexus代理转换',
          _ => '点账号「使用」接管',
        },
        icon: Icons.route_outlined,
      ),
    ];

    final List<StatPoint> series = data.stats.series;
    final List<String> labels = [for (final p in series) p.shortLabel];
    final int days = data.stats.days;

    // 「总 Token」增长曲线：把每日 token 按天累加，最后一点即窗口内累计总量。
    final List<double> cumulativeTokens = <double>[];
    {
      double running = 0;
      for (final p in series) {
        running += p.tokens.toDouble();
        cumulativeTokens.add(running);
      }
    }

    final AppSettings? s = _settings;
    final bool showWeather = s?.showWeather ?? false;
    final bool showPoem = s?.showPoem ?? false;

    return ListView(
      padding: EdgeInsets.zero,
      children: [
        if (showPoem) ...[
          PoemBanner(
            client: widget.client,
            category: s!.poemCategory.isEmpty ? null : s.poemCategory,
          ),
          const SizedBox(height: 16),
        ],
        if (showWeather) ...[
          WeatherCard(
            client: widget.client,
            // 自动定位时不传城市，由后端按 IP 定位；手动且填了城市才传。
            city: (s!.weatherAutoLocate || s.weatherCity.isEmpty)
                ? null
                : s.weatherCity,
          ),
          const SizedBox(height: 16),
        ],
        _SidecarCard(sidecar: sidecar),
        const SizedBox(height: 16),
        LayoutBuilder(
          builder: (context, constraints) {
            final double w = constraints.maxWidth;
            final int cols = w >= 1040 ? 4 : (w >= 700 ? 2 : 1);
            const double spacing = 16;
            final double cardW = (w - (cols - 1) * spacing) / cols;
            return Wrap(
              spacing: spacing,
              runSpacing: spacing,
              children: [
                for (final card in metrics)
                  SizedBox(width: cardW, child: card),
              ],
            );
          },
        ),
        const SizedBox(height: 22),
        _SectionTitle(
          title: '数据趋势',
          subtitle: days > 0 ? '近 $days 天（按天）' : '按天',
        ),
        const SizedBox(height: 14),
        LayoutBuilder(
          builder: (context, constraints) {
            final double w = constraints.maxWidth;
            final bool two = w >= 880;
            const double spacing = 16;
            final double cardW = two ? (w - spacing) / 2 : w;
            final charts = <Widget>[
              TrendChartCard(
                title: 'Token 用量',
                subtitle: '每日消耗的 token 总量',
                labels: labels,
                series: [
                  ChartSeries(
                    name: 'Token',
                    color: ferryAccent,
                    values: [for (final p in series) p.tokens.toDouble()],
                  ),
                ],
              ),
              TrendChartCard(
                title: '总 Token（累计）',
                subtitle: '按天累加的 token 总量',
                labels: labels,
                series: [
                  ChartSeries(
                    name: '累计 Token',
                    color: const Color(0xFF22D3EE),
                    values: cumulativeTokens,
                  ),
                ],
              ),
              TrendChartCard(
                title: '请求数',
                subtitle: '每日成功 / 失败请求',
                labels: labels,
                series: [
                  ChartSeries(
                    name: '成功',
                    color: const Color(0xFF34D399),
                    values: [for (final p in series) p.succeeded.toDouble()],
                  ),
                  ChartSeries(
                    name: '失败',
                    color: const Color(0xFFF87171),
                    values: [for (final p in series) p.failed.toDouble()],
                  ),
                ],
              ),
              TrendChartCard(
                title: '账号变动',
                subtitle: '每日新增 / 删除账号',
                labels: labels,
                series: [
                  ChartSeries(
                    name: '新增',
                    color: ferryAccent,
                    values: [for (final p in series) p.accountsAdded.toDouble()],
                  ),
                  ChartSeries(
                    name: '删除',
                    color: const Color(0xFFFB923C),
                    values: [
                      for (final p in series) p.accountsDeleted.toDouble(),
                    ],
                  ),
                ],
              ),
            ];
            return Wrap(
              spacing: spacing,
              runSpacing: spacing,
              children: [
                for (final c in charts) SizedBox(width: cardW, child: c),
              ],
            );
          },
        ),
        if (data.stats.providerUsage.isNotEmpty) ...[
          const SizedBox(height: 22),
          const _SectionTitle(
            title: 'Token 用量校验',
            subtitle: '上游上报 vs Codexus本地估算 · 识别中转掺假',
          ),
          const SizedBox(height: 14),
          _TokenAuditCard(usage: data.stats.providerUsage),
        ],
      ],
    );
  }
}

/// 各供应商/账号的 token 用量对比卡（上游上报 vs 本地估算 + 掺假提示）。
class _TokenAuditCard extends StatelessWidget {
  const _TokenAuditCard({required this.usage});

  final List<ProviderUsage> usage;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final bool anySuspect = usage.any((u) => u.suspect);
    return GlassSurface(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                anySuspect ? Icons.gpp_maybe : Icons.verified_user_outlined,
                size: 18,
                color: anySuspect
                    ? const Color(0xFFFB923C)
                    : const Color(0xFF34D399),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  anySuspect
                      ? '检测到上游上报明显高于本地估算的供应商,请留意中转是否虚报 token'
                      : '各供应商上报与本地估算基本一致',
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.65),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 14),
          for (final u in usage) ...[
            _TokenAuditRow(usage: u),
            if (u != usage.last) ...[
              const SizedBox(height: 10),
              Divider(height: 1, color: onSurface.withValues(alpha: 0.08)),
              const SizedBox(height: 10),
            ],
          ],
        ],
      ),
    );
  }
}

class _TokenAuditRow extends StatelessWidget {
  const _TokenAuditRow({required this.usage});

  final ProviderUsage usage;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final ratio = usage.ratio;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                usage.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: text.titleSmall?.copyWith(fontWeight: FontWeight.w600),
              ),
            ),
            const SizedBox(width: 8),
            if (usage.isPool)
              const _Tag(text: '官方', color: Color(0xFF8B5CF6))
            else if (usage.suspect)
              const _Tag(text: '疑似掺假', color: Color(0xFFF87171))
            else
              const _Tag(text: '正常', color: Color(0xFF34D399)),
          ],
        ),
        const SizedBox(height: 6),
        Text(
          '请求 ${usage.requests} · 上报 ${_compactNum(usage.reportedTotal)} tokens'
          '${usage.isPool ? '（官方计量,可信）' : ' · 本地估算 ${_compactNum(usage.estTotal)}'}'
          '${ratio != null && !usage.isPool ? ' · 比值 ${ratio.toStringAsFixed(2)}×' : ''}',
          style: text.bodySmall?.copyWith(
            color: onSurface.withValues(alpha: 0.6),
          ),
        ),
      ],
    );
  }
}

/// 大数字紧凑显示（1.2k / 3.4m）。
String _compactNum(int v) {
  if (v >= 1000000000) return '${(v / 1e9).toStringAsFixed(1)}b';
  if (v >= 1000000) return '${(v / 1e6).toStringAsFixed(1)}m';
  if (v >= 1000) return '${(v / 1e3).toStringAsFixed(1)}k';
  return '$v';
}

/// 冷却剩余秒数的人性化展示（用于账号「不可用 · 冷却中 …」）。
String _fmtCooldown(int secs) {
  if (secs >= 3600) return '${(secs / 3600).toStringAsFixed(1)}h';
  if (secs >= 60) return '${(secs / 60).ceil()}m';
  return '${secs}s';
}

class _SectionTitle extends StatelessWidget {
  const _SectionTitle({required this.title, required this.subtitle});

  final String title;
  final String subtitle;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.baseline,
      textBaseline: TextBaseline.alphabetic,
      children: [
        Text(
          title,
          style: text.titleLarge?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.3,
          ),
        ),
        const SizedBox(width: 10),
        Text(
          subtitle,
          style: text.bodySmall?.copyWith(
            color: onSurface.withValues(alpha: 0.5),
          ),
        ),
      ],
    );
  }
}

class _SidecarCard extends StatelessWidget {
  const _SidecarCard({required this.sidecar});

  final SidecarController sidecar;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final bool running = sidecar.isRunning;
    return GlassSurface(
      padding: const EdgeInsets.all(18),
      child: Row(
        children: [
          Container(
            width: 46,
            height: 46,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(FerryRadii.control),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: running
                    ? [const Color(0x4D34D399), const Color(0x1434D399)]
                    : [
                        ferryAccent.withValues(alpha: 0.28),
                        ferryAccent.withValues(alpha: 0.08),
                      ],
              ),
              border: Border.all(
                color: running
                    ? const Color(0x6634D399)
                    : ferryAccent.withValues(alpha: 0.3),
              ),
            ),
            child: Icon(
              running ? Icons.bolt : Icons.power_settings_new,
              color: running ? const Color(0xFF34D399) : ferryAccent,
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  '本地后端 Sidecar · ${_sidecarLabel(sidecar.status)}',
                  style: text.titleMedium?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  sidecar.message ?? '由 GUI 启动 / 停止 ferry-daemon serve',
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.6),
                  ),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          FilledButton.icon(
            onPressed: running ? null : sidecar.start,
            icon: const Icon(Icons.play_arrow, size: 18),
            label: const Text('启动'),
          ),
          const SizedBox(width: 8),
          OutlinedButton.icon(
            onPressed: running ? sidecar.stop : null,
            icon: const Icon(Icons.stop, size: 18),
            label: const Text('停止'),
          ),
        ],
      ),
    );
  }
}

class _ProvidersPage extends StatefulWidget {
  const _ProvidersPage({
    required this.providers,
    required this.codexStatus,
    required this.activeProvider,
    required this.client,
    required this.onTakeover,
    required this.onChanged,
  });

  final List<ProviderPreset> providers;
  final CodexStatus codexStatus;
  final ActiveProvider activeProvider;
  final IpcClient client;
  final Future<void> Function(ProviderPreset provider) onTakeover;
  final VoidCallback onChanged;

  @override
  State<_ProvidersPage> createState() => _ProvidersPageState();
}

class _ProvidersPageState extends State<_ProvidersPage> {
  bool _busy = false;

  void _notify(String message) {
    if (!mounted) {
      return;
    }
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  /// 供应商页「添加账号」：打开账号弹窗并预选该供应商（API Key 分页）。
  /// 添加成功后自动「设为当前并接管 Codex」，做到即开即用。
  Future<void> _addAccountForProvider(ProviderPreset provider) async {
    final added = await showAddAccountDialog(
      context,
      widget.client,
      providers: widget.providers,
      initialProviderId: provider.id,
    );
    if (added != true) {
      return;
    }
    try {
      await widget.onTakeover(provider);
    } catch (error) {
      _notify('账号已添加，但接管 Codex 失败：$error');
      widget.onChanged();
    }
  }

  Future<void> _addOrEdit({ProviderPreset? existing}) async {
    final result = await showDialog<_ProviderFormResult>(
      context: context,
      builder: (_) => _ProviderFormDialog(existing: existing),
    );
    if (result == null) {
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.client.upsertProvider(result.body);
      final key = result.apiKey.trim();
      if (key.isNotEmpty) {
        await widget.client.setProviderApiKey(result.body['id'] as String, key);
      }
      _notify('已保存供应商：${result.body['name']}');
      widget.onChanged();
    } catch (error) {
      _notify('保存失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _setKey(ProviderPreset provider) async {
    final controller = TextEditingController();
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('为 ${provider.name} 设置 API Key'),
        content: TextField(
          controller: controller,
          obscureText: true,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'API Key',
            hintText: '保存到本地凭据文件（0600 权限）',
            border: OutlineInputBorder(),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('保存'),
          ),
        ],
      ),
    );
    final key = controller.text.trim();
    controller.dispose();
    if (confirmed != true || key.isEmpty) {
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.client.setProviderApiKey(provider.id, key);
      _notify('已保存 ${provider.name} 的 API Key');
      widget.onChanged();
    } catch (error) {
      _notify('保存 Key 失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _delete(ProviderPreset provider) async {
    final bool restore = provider.isOverride;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(restore ? '恢复默认' : '删除自定义供应商'),
        content: Text(
          restore
              ? '确定把「${provider.name}」恢复为内置默认设置吗？（已保存的 API Key 会保留）'
              : '确定删除「${provider.name}」吗？其保存的 API Key 也会一并清除。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(restore ? '恢复默认' : '删除'),
          ),
        ],
      ),
    );
    if (confirmed != true) {
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.client.deleteProvider(provider.id);
      _notify(restore ? '已恢复 ${provider.name} 为默认' : '已删除 ${provider.name}');
      widget.onChanged();
    } catch (error) {
      _notify(restore ? '恢复失败：$error' : '删除失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _export() async {
    setState(() => _busy = true);
    try {
      final providers = await widget.client.exportProviders();
      if (providers.isEmpty) {
        _notify('暂无自定义供应商可导出');
        return;
      }
      final path = await ProvidersIo.exportToDownloads(providers);
      _notify('已导出 ${providers.length} 个自定义供应商到：$path');
    } catch (error) {
      _notify('导出失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  Future<void> _import() async {
    final controller = TextEditingController();
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('导入供应商配置'),
        content: SizedBox(
          width: 520,
          child: TextField(
            controller: controller,
            autofocus: true,
            maxLines: 12,
            decoration: const InputDecoration(
              hintText: '粘贴供应商 JSON（数组或 {"providers": [...]}）',
              border: OutlineInputBorder(),
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('导入'),
          ),
        ],
      ),
    );
    final raw = controller.text.trim();
    controller.dispose();
    if (confirmed != true || raw.isEmpty) {
      return;
    }
    setState(() => _busy = true);
    try {
      final providers = ProvidersIo.parseImport(raw);
      final count = await widget.client.importProviders(providers);
      _notify('已导入 $count 个供应商');
      widget.onChanged();
    } catch (error) {
      _notify('导入失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final String activeId = widget.activeProvider.providerId ?? '';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        GlassSurface(
          padding: const EdgeInsets.all(16),
          child: Row(
            children: [
              Icon(
                widget.codexStatus.exists
                    ? Icons.verified_outlined
                    : Icons.settings_outlined,
                color: widget.codexStatus.exists
                    ? const Color(0xFF34D399)
                    : onSurface.withValues(alpha: 0.7),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      widget.codexStatus.exists ? 'Codex 配置已存在' : 'Codex 配置未创建',
                      style: text.titleSmall?.copyWith(
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    Text(
                      widget.codexStatus.configPath,
                      style: text.bodySmall?.copyWith(
                        color: onSurface.withValues(alpha: 0.55),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            FilledButton.icon(
              onPressed: _busy ? null : () => _addOrEdit(),
              icon: const Icon(Icons.add),
              label: const Text('新增自定义供应商'),
            ),
            OutlinedButton.icon(
              onPressed: _busy ? null : _import,
              icon: const Icon(Icons.file_download_outlined),
              label: const Text('导入'),
            ),
            OutlinedButton.icon(
              onPressed: _busy ? null : _export,
              icon: const Icon(Icons.file_upload_outlined),
              label: const Text('导出'),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Expanded(
          child: widget.providers.isEmpty
              ? const EmptyState(
                  icon: Icons.hub_outlined,
                  title: '暂无供应商',
                  message: '请确认 ferry-daemon 已启动并暴露 /ipc/providers。',
                )
              : ListView(
                  padding: EdgeInsets.zero,
                  children: _groupedChildren(context, activeId),
                ),
        ),
      ],
    );
  }

  /// 按「直连 / 中转」分组渲染供应商卡片。
  List<Widget> _groupedChildren(BuildContext context, String activeId) {
    final direct = widget.providers.where((p) => !p.isRelay).toList();
    final relay = widget.providers.where((p) => p.isRelay).toList();
    final children = <Widget>[];

    void addGroup(String title, String subtitle, List<ProviderPreset> list) {
      if (list.isEmpty) {
        return;
      }
      if (children.isNotEmpty) {
        children.add(const SizedBox(height: 18));
      }
      children.add(_GroupHeader(title: title, subtitle: subtitle, count: list.length));
      children.add(const SizedBox(height: 10));
      for (var i = 0; i < list.length; i++) {
        if (i > 0) {
          children.add(const SizedBox(height: 12));
        }
        children.add(_card(list[i], activeId));
      }
    }

    addGroup('直连供应商', '调用供应商官方接口', direct);
    addGroup('中转 / 聚合', '第三方代理站,经Codexus校验 token 真实性', relay);
    return children;
  }

  Widget _card(ProviderPreset provider, String activeId) {
    return _ProviderCard(
      provider: provider,
      active: provider.id == activeId,
      busy: _busy,
      onAddAccount: () => _addAccountForProvider(provider),
      onTakeover: () => widget.onTakeover(provider),
      onSetKey: () => _setKey(provider),
      onEdit: () => _addOrEdit(existing: provider),
      onDelete: () => _delete(provider),
    );
  }
}

class _GroupHeader extends StatelessWidget {
  const _GroupHeader({
    required this.title,
    required this.subtitle,
    required this.count,
  });

  final String title;
  final String subtitle;
  final int count;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.baseline,
      textBaseline: TextBaseline.alphabetic,
      children: [
        Text(
          title,
          style: text.titleMedium?.copyWith(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.2,
          ),
        ),
        const SizedBox(width: 8),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 1),
          decoration: BoxDecoration(
            color: onSurface.withValues(alpha: 0.08),
            borderRadius: BorderRadius.circular(FerryRadii.small),
          ),
          child: Text(
            '$count',
            style: text.bodySmall?.copyWith(
              color: onSurface.withValues(alpha: 0.6),
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            subtitle,
            style: text.bodySmall?.copyWith(
              color: onSurface.withValues(alpha: 0.5),
            ),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    );
  }
}

class _ProviderCard extends StatelessWidget {
  const _ProviderCard({
    required this.provider,
    required this.active,
    required this.busy,
    required this.onAddAccount,
    required this.onTakeover,
    required this.onSetKey,
    required this.onEdit,
    required this.onDelete,
  });

  final ProviderPreset provider;
  final bool active;
  final bool busy;
  final VoidCallback onAddAccount;
  final VoidCallback onTakeover;
  final VoidCallback onSetKey;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return GlassSurface(
      strong: active,
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.hub, color: onSurface.withValues(alpha: 0.85)),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  provider.name,
                  style: text.titleMedium?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              if (active) ...[
                const _Tag(text: '运行中', color: Color(0xFF34D399)),
                const SizedBox(width: 6),
              ],
              if (provider.isRelay) ...[
                const _Tag(text: '中转', color: Color(0xFFFB923C)),
                const SizedBox(width: 6),
              ],
              _ProviderBadge(builtin: provider.builtin),
            ],
          ),
          const SizedBox(height: 8),
          Text(
            provider.baseUrl,
            style: text.bodySmall?.copyWith(
              color: onSurface.withValues(alpha: 0.55),
              fontFamily: 'monospace',
            ),
          ),
          const SizedBox(height: 2),
          Text(
            '默认模型：${provider.defaultModel} · ${provider.api}',
            style: text.bodyMedium,
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              FilledButton.icon(
                onPressed: busy ? null : onAddAccount,
                icon: const Icon(Icons.person_add_alt_1, size: 18),
                label: const Text('添加账号'),
              ),
              if (active) ...[
                const SizedBox(width: 8),
                Text(
                  '已接管',
                  style: text.bodySmall?.copyWith(
                    color: const Color(0xFF34D399),
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
              const Spacer(),
              PopupMenuButton<String>(
                enabled: !busy,
                tooltip: '更多操作',
                icon: Icon(
                  Icons.more_horiz,
                  color: onSurface.withValues(alpha: 0.7),
                ),
                onSelected: (value) {
                  switch (value) {
                    case 'takeover':
                      onTakeover();
                    case 'key':
                      onSetKey();
                    case 'edit':
                      onEdit();
                    case 'delete':
                      onDelete();
                  }
                },
                itemBuilder: (context) => [
                  const PopupMenuItem(
                    value: 'takeover',
                    child: Text('启用并接管 Codex'),
                  ),
                  const PopupMenuItem(value: 'key', child: Text('设置 API Key')),
                  const PopupMenuItem(value: 'edit', child: Text('编辑')),
                  if (provider.isPureCustom)
                    const PopupMenuItem(value: 'delete', child: Text('删除'))
                  else if (provider.isOverride)
                    const PopupMenuItem(
                      value: 'delete',
                      child: Text('恢复默认'),
                    ),
                ],
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Tag extends StatelessWidget {
  const _Tag({required this.text, required this.color});

  final String text;
  final Color color;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: color.withValues(alpha: 0.4)),
      ),
      child: Text(
        text,
        style: TextStyle(color: color, fontSize: 11, fontWeight: FontWeight.w600),
      ),
    );
  }
}

class _ProviderBadge extends StatelessWidget {
  const _ProviderBadge({required this.builtin});

  final bool builtin;

  @override
  Widget build(BuildContext context) {
    final Color color = builtin
        ? const Color(0xFF8B5CF6)
        : const Color(0xFF38BDF8);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: color.withValues(alpha: 0.4)),
      ),
      child: Text(
        builtin ? '内置' : '自定义',
        style: TextStyle(color: color, fontSize: 11, fontWeight: FontWeight.w600),
      ),
    );
  }
}

class _ProviderFormResult {
  const _ProviderFormResult({required this.body, required this.apiKey});

  final Map<String, dynamic> body;
  final String apiKey;
}

class _ProviderFormDialog extends StatefulWidget {
  const _ProviderFormDialog({this.existing});

  final ProviderPreset? existing;

  @override
  State<_ProviderFormDialog> createState() => _ProviderFormDialogState();
}

class _ProviderFormDialogState extends State<_ProviderFormDialog> {
  late final TextEditingController _id;
  late final TextEditingController _name;
  late final TextEditingController _baseUrl;
  late final TextEditingController _defaultModel;
  late final TextEditingController _apiKeyEnv;
  final TextEditingController _apiKey = TextEditingController();
  String _api = 'chat';
  String _kind = 'direct';

  bool get _isEdit => widget.existing != null;

  @override
  void initState() {
    super.initState();
    final e = widget.existing;
    _id = TextEditingController(text: e?.id ?? '');
    _name = TextEditingController(text: e?.name ?? '');
    _baseUrl = TextEditingController(text: e?.baseUrl ?? '');
    _defaultModel = TextEditingController(text: e?.defaultModel ?? '');
    _apiKeyEnv = TextEditingController(text: e?.apiKeyEnv.join(', ') ?? '');
    _api = e?.api == 'responses' ? 'responses' : 'chat';
    _kind = e?.kind == 'relay' ? 'relay' : 'direct';
  }

  @override
  void dispose() {
    _id.dispose();
    _name.dispose();
    _baseUrl.dispose();
    _defaultModel.dispose();
    _apiKeyEnv.dispose();
    _apiKey.dispose();
    super.dispose();
  }

  void _submit() {
    final id = _id.text.trim();
    final name = _name.text.trim();
    final baseUrl = _baseUrl.text.trim();
    final defaultModel = _defaultModel.text.trim();
    if (id.isEmpty || name.isEmpty || baseUrl.isEmpty || defaultModel.isEmpty) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('id / 名称 / base_url / 默认模型 均为必填')),
      );
      return;
    }
    final envs = _apiKeyEnv.text
        .split(',')
        .map((e) => e.trim())
        .where((e) => e.isNotEmpty)
        .toList();
    final body = <String, dynamic>{
      'id': id,
      'name': name,
      'base_url': baseUrl,
      'api': _api,
      'default_model': defaultModel,
      'api_key_env': envs,
      'kind': _kind,
    };
    // 编辑时保留已有模型别名（表单不编辑别名，避免覆盖内置预设时丢失 codex 映射）。
    final existing = widget.existing;
    if (existing != null && existing.aliases.isNotEmpty) {
      body['aliases'] = [
        for (final a in existing.aliases) {'from': a.from, 'to': a.to},
      ];
    }
    Navigator.of(context).pop(
      _ProviderFormResult(body: body, apiKey: _apiKey.text),
    );
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(_isEdit ? '编辑供应商' : '新增自定义供应商'),
      content: SizedBox(
        width: 520,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: _id,
                enabled: !_isEdit,
                decoration: const InputDecoration(
                  labelText: 'id（字母/数字/-/_，不可与内置预设重名）',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _name,
                decoration: const InputDecoration(
                  labelText: '名称',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _baseUrl,
                decoration: const InputDecoration(
                  labelText: 'Base URL（如 https://api.x.com/v1）',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _defaultModel,
                decoration: const InputDecoration(
                  labelText: '默认模型',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              DropdownButtonFormField<String>(
                initialValue: _api,
                dropdownColor: ferryMenuColor(context),
                borderRadius: BorderRadius.circular(FerryRadii.control),
                decoration: const InputDecoration(
                  labelText: '协议类型',
                  border: OutlineInputBorder(),
                ),
                items: const [
                  DropdownMenuItem(
                    value: 'chat',
                    child: Text('chat（Chat Completions）'),
                  ),
                  DropdownMenuItem(
                    value: 'responses',
                    child: Text('responses（Responses）'),
                  ),
                ],
                onChanged: (value) => setState(() => _api = value ?? 'chat'),
              ),
              const SizedBox(height: 12),
              DropdownButtonFormField<String>(
                initialValue: _kind,
                dropdownColor: ferryMenuColor(context),
                borderRadius: BorderRadius.circular(FerryRadii.control),
                decoration: const InputDecoration(
                  labelText: '分组类型',
                  border: OutlineInputBorder(),
                ),
                items: const [
                  DropdownMenuItem(
                    value: 'direct',
                    child: Text('直连（供应商官方接口）'),
                  ),
                  DropdownMenuItem(
                    value: 'relay',
                    child: Text('中转（第三方聚合 / 代理站）'),
                  ),
                ],
                onChanged: (value) => setState(() => _kind = value ?? 'direct'),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _apiKeyEnv,
                decoration: const InputDecoration(
                  labelText: 'API Key 环境变量（可选，逗号分隔）',
                  border: OutlineInputBorder(),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                controller: _apiKey,
                obscureText: true,
                decoration: const InputDecoration(
                  labelText: 'API Key（可选，保存到本地凭据文件）',
                  border: OutlineInputBorder(),
                ),
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(onPressed: _submit, child: const Text('保存')),
      ],
    );
  }
}

class _AccountsPage extends StatefulWidget {
  const _AccountsPage({
    required this.accounts,
    required this.providers,
    required this.stats,
    required this.client,
    required this.onChanged,
  });

  final List<AccountSummary> accounts;
  final List<ProviderPreset> providers;
  final Stats stats;
  final IpcClient client;
  final VoidCallback onChanged;

  @override
  State<_AccountsPage> createState() => _AccountsPageState();
}

class _AccountsPageState extends State<_AccountsPage> {
  bool _busy = false;

  /// 额度探测进行中（手动或自动），避免重叠请求上游。
  bool _probingQuota = false;

  /// 逐账号官方额度（键为账号稳定 id，与账号池快照 key 一致）。
  Map<String, PoolAccountStatus> _quota = const {};

  /// 每分钟自动刷新一次额度（异步、静默，不打断用户、不弹提示）。
  Timer? _quotaTimer;
  static const _quotaInterval = Duration(minutes: 1);

  @override
  void initState() {
    super.initState();
    // 进页：先用缓存秒显，再后台异步探测一次上游最新额度（无需用户手点）。
    _loadQuota();
    _autoRefreshQuota();
    _quotaTimer = Timer.periodic(_quotaInterval, (_) => _autoRefreshQuota());
  }

  @override
  void dispose() {
    _quotaTimer?.cancel();
    super.dispose();
  }

  void _notify(String message) {
    if (!mounted) {
      return;
    }
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  /// 读取账号池快照里的已缓存额度（不触发上游探测）。
  Future<void> _loadQuota() async {
    try {
      final resp = await widget.client.pool();
      if (!mounted) return;
      setState(() {
        _quota = {for (final a in resp.snapshot.accounts) a.key: a};
      });
    } catch (_) {
      // 后端未起或无池账号时静默忽略。
    }
  }

  /// 异步静默刷新额度：进页与每分钟定时触发，不锁 UI、不弹提示
  /// （「不用每次手点看额度」）。
  Future<void> _autoRefreshQuota() async {
    if (!mounted || _probingQuota) {
      return;
    }
    _probingQuota = true;
    try {
      final resp = await widget.client.refreshPoolQuota();
      if (!mounted) return;
      setState(() {
        _quota = {for (final a in resp.snapshot.accounts) a.key: a};
      });
    } catch (_) {
      // 自动刷新失败保持旧额度，静默（手动刷新才提示错误）。
    } finally {
      _probingQuota = false;
    }
  }

  /// 主动探测刷新各账号 5h/7d 额度（仿 Cockpit Tools 的额度刷新，带忙碌态+提示）。
  Future<void> _refreshQuota() async {
    if (_probingQuota) {
      return;
    }
    _probingQuota = true;
    setState(() => _busy = true);
    try {
      final resp = await widget.client.refreshPoolQuota();
      if (!mounted) return;
      setState(() {
        _quota = {for (final a in resp.snapshot.accounts) a.key: a};
      });
      _notify('已刷新额度');
    } catch (error) {
      _notify('刷新额度失败：$error');
    } finally {
      _probingQuota = false;
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  /// 判断账号是否可用：有池状态时按 token/健康/额度判定，否则按令牌是否过期；
  /// 返回 (不可用, 原因)。用于「账号不可用时也显示」并标注原因（仿 cockpit）。
  ({bool unavailable, String? reason}) _availability(AccountSummary a) {
    final st = _quota[a.id];
    if (st != null) {
      if (!st.tokenPresent) {
        return (unavailable: true, reason: '缺少有效 token');
      }
      if (!st.healthy) {
        if (st.coolingDown && st.cooldownRemainingSecs > 0) {
          return (unavailable: true, reason: '冷却中 ${_fmtCooldown(st.cooldownRemainingSecs)}');
        }
        return (unavailable: true, reason: st.lastError ?? '近期请求失败');
      }
      if (st.quotaExhausted) {
        return (unavailable: true, reason: '额度已用尽');
      }
    }
    final exp = a.expiresAt;
    if (exp != null && exp.isBefore(DateTime.now())) {
      return (unavailable: true, reason: '令牌已过期');
    }
    return (unavailable: false, reason: null);
  }

  /// 当前可用账号数（用于顶部概述）。
  int get _availableCount {
    var n = 0;
    for (final a in widget.accounts) {
      if (!_availability(a).unavailable) {
        n++;
      }
    }
    return n;
  }

  Future<void> _addAccount() async {
    final added = await showAddAccountDialog(
      context,
      widget.client,
      providers: widget.providers,
    );
    if (added == true) {
      _notify('账号已添加');
      widget.onChanged();
    }
  }

  /// 「使用」账号：统一走后端**自动识别路由**端点（`/ipc/accounts/{id}/use`）。
  /// OAuth(ChatGPT) 直连官方、API Key/中转/供应商经本地代理，由后端按账号类型
  /// 自动分流，前端不再手选「供应商 / 账号池」模式。
  Future<void> _useAccount(AccountSummary account) async {
    setState(() => _busy = true);
    try {
      final result = await widget.client.useAccount(
        account.id,
        model: account.model,
      );
      final where = result.mode == 'direct' ? '直连官方' : '经本地代理';
      final model = (result.model ?? '').trim();
      final modelText = model.isEmpty ? '' : '到 $model ';
      _notify('已使用 ${account.displayName} $modelText· $where 并接管 Codex');
      widget.onChanged();
    } catch (error) {
      _notify('使用失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  /// 编辑账号：自定义名称 / 标签 / 备注 / Key。
  Future<void> _editAccount(AccountSummary account) async {
    final saved = await showEditAccountDialog(context, widget.client, account);
    if (saved == true) {
      _notify('已更新 ${account.displayName}');
      widget.onChanged();
    }
  }

  Future<void> _delete(AccountSummary account) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('删除账号'),
        content: Text(
          '确定删除「${account.displayName}」吗？该账号的本地凭据文件也会一并清除。',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('取消'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('删除'),
          ),
        ],
      ),
    );
    if (confirmed != true) {
      return;
    }
    setState(() => _busy = true);
    try {
      await widget.client.deleteAccount(Uri.encodeComponent(account.id));
      _notify('已删除 ${account.displayName}');
      widget.onChanged();
    } catch (error) {
      _notify('删除失败：$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final accounts = widget.accounts;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        _Toolbar(
          count: accounts.length,
          busy: _busy,
          onAdd: _addAccount,
          onRefreshQuota: _refreshQuota,
        ),
        if (accounts.isNotEmpty) ...[
          const SizedBox(height: 14),
          _buildOverview(context),
        ],
        const SizedBox(height: 14),
        if (accounts.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 28),
            child: EmptyState(
              icon: Icons.key_off,
              title: '暂无账号',
              message:
                  '点「添加账号」用 ChatGPT 登录、粘贴 API Key / Token，或从本机 Codex 导入。',
              action: FilledButton.icon(
                onPressed: _busy ? null : _addAccount,
                icon: const Icon(Icons.add),
                label: const Text('添加账号'),
              ),
            ),
          )
        else
          LayoutBuilder(
            builder: (context, constraints) {
              final double w = constraints.maxWidth;
              final int cols = w >= 1040 ? 3 : (w >= 680 ? 2 : 1);
              const double spacing = 16;
              final double cardW = (w - (cols - 1) * spacing) / cols;
              return Wrap(
                spacing: spacing,
                runSpacing: spacing,
                children: [
                  for (final account in accounts)
                    SizedBox(
                      width: cardW,
                      child: Builder(
                        builder: (context) {
                          final avail = _availability(account);
                          return AccountCard(
                            account: account,
                            busy: _busy,
                            status: _quota[account.id],
                            unavailable: avail.unavailable,
                            unavailableReason: avail.reason,
                            onUse: (account.isOAuth || account.vendorBound)
                                ? () => _useAccount(account)
                                : null,
                            onEdit: () => _editAccount(account),
                            onDelete: () => _delete(account),
                          );
                        },
                      ),
                    ),
                ],
              );
            },
          ),
      ],
    );
  }

  /// 顶部「概述」：Token 总量 + 今日用量 + 账号可用概况（用户进账号页即可一眼看到，
  /// 不必切到仪表盘）。
  Widget _buildOverview(BuildContext context) {
    final StatsTotals t = widget.stats.totals;
    final List<StatPoint> series = widget.stats.series;
    final StatPoint? today = series.isNotEmpty ? series.last : null;
    final int todayTokens = today?.tokens ?? 0;
    final int total = widget.accounts.length;
    final int avail = _availableCount;
    final bool allOk = avail >= total;
    return GlassSurface(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
      child: Row(
        children: [
          Expanded(
            child: _OverviewCell(
              icon: Icons.token_outlined,
              label: '累计 Token',
              value: _compactNum(t.totalTokens),
              sub: '输入 ${_compactNum(t.inputTokens)} · 输出 ${_compactNum(t.outputTokens)}',
            ),
          ),
          const _OverviewDivider(),
          Expanded(
            child: _OverviewCell(
              icon: Icons.today_outlined,
              label: '今日 Token',
              value: _compactNum(todayTokens),
              sub: todayTokens > 0
                  ? '输入 ${_compactNum(today?.inputTokens ?? 0)} · 输出 ${_compactNum(today?.outputTokens ?? 0)}'
                  : '今日暂无对话',
            ),
          ),
          const _OverviewDivider(),
          Expanded(
            child: _OverviewCell(
              icon: Icons.verified_user_outlined,
              label: '可用账号',
              value: '$avail / $total',
              sub: allOk ? '全部可用' : '${total - avail} 个不可用',
              accent: allOk
                  ? const Color(0xFF34D399)
                  : const Color(0xFFFBBF24),
            ),
          ),
        ],
      ),
    );
  }
}

/// 概述单元格（图标 + 标签 + 主数值 + 副文案）。
class _OverviewCell extends StatelessWidget {
  const _OverviewCell({
    required this.icon,
    required this.label,
    required this.value,
    required this.sub,
    this.accent,
  });

  final IconData icon;
  final String label;
  final String value;
  final String sub;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final Color c = accent ?? ferryAccent;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            Icon(icon, size: 14, color: c.withValues(alpha: 0.9)),
            const SizedBox(width: 6),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: text.bodySmall?.copyWith(
                  color: onSurface.withValues(alpha: 0.55),
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        Text(
          value,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: text.titleLarge?.copyWith(
            fontWeight: FontWeight.w800,
            letterSpacing: -0.5,
            color: accent,
          ),
        ),
        const SizedBox(height: 2),
        Text(
          sub,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: text.bodySmall?.copyWith(
            color: onSurface.withValues(alpha: 0.45),
          ),
        ),
      ],
    );
  }
}

class _OverviewDivider extends StatelessWidget {
  const _OverviewDivider();

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Container(
      width: 1,
      height: 38,
      margin: const EdgeInsets.symmetric(horizontal: 14),
      color: onSurface.withValues(alpha: 0.1),
    );
  }
}

class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.count,
    required this.busy,
    required this.onAdd,
    this.onRefreshQuota,
  });

  final int count;
  final bool busy;
  final VoidCallback onAdd;
  final VoidCallback? onRefreshQuota;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return GlassSurface(
      padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
      child: Row(
        children: [
          Icon(Icons.key, color: onSurface.withValues(alpha: 0.85)),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  '管理 Codex 登录账号',
                  style: text.titleMedium?.copyWith(
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  count == 0 ? '尚未添加账号' : '共 $count 个账号',
                  style: text.bodySmall?.copyWith(
                    color: onSurface.withValues(alpha: 0.55),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (onRefreshQuota != null) ...[
            OutlinedButton.icon(
              onPressed: busy ? null : onRefreshQuota,
              icon: const Icon(Icons.speed, size: 18),
              label: const Text('刷新额度'),
            ),
            const SizedBox(width: 10),
          ],
          FilledButton.icon(
            onPressed: busy ? null : onAdd,
            icon: const Icon(Icons.add),
            label: const Text('添加账号'),
          ),
        ],
      ),
    );
  }
}

class _SessionsPage extends StatefulWidget {
  const _SessionsPage({required this.sessions, required this.client});

  final List<SessionRecord> sessions;
  final IpcClient client;

  @override
  State<_SessionsPage> createState() => _SessionsPageState();
}

class _SessionsPageState extends State<_SessionsPage> {
  final TextEditingController _searchController = TextEditingController();
  String _query = '';
  bool _exporting = false;

  @override
  void dispose() {
    _searchController.dispose();
    super.dispose();
  }

  List<SessionRecord> get _filtered {
    final query = _query.trim().toLowerCase();
    if (query.isEmpty) {
      return widget.sessions;
    }
    return widget.sessions.where((s) {
      final haystack = [s.title, s.cwd, s.sessionId].join(' ').toLowerCase();
      return haystack.contains(query);
    }).toList();
  }

  Future<void> _export(bool asJson) async {
    final items = _filtered;
    if (items.isEmpty) {
      _notify('当前筛选下没有可导出的会话');
      return;
    }
    setState(() => _exporting = true);
    try {
      final path = asJson
          ? await SessionExporter.exportJson(items)
          : await SessionExporter.exportMarkdown(items);
      _notify('已导出 ${items.length} 条到：$path');
    } catch (error) {
      _notify('导出失败：$error');
    } finally {
      if (mounted) {
        setState(() => _exporting = false);
      }
    }
  }

  void _notify(String message) {
    if (!mounted) {
      return;
    }
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(SnackBar(content: Text(message)));
  }

  void _showDetail(SessionRecord session) {
    showDialog<void>(
      context: context,
      builder: (context) =>
          _SessionDetailDialog(client: widget.client, session: session),
    );
  }

  @override
  Widget build(BuildContext context) {
    if (widget.sessions.isEmpty) {
      return const EmptyState(
        icon: Icons.history_toggle_off,
        title: '暂无会话记录',
        message: '用 Codex 跑过对话后，这里会自动读取本地 rollout 会话与真实 token 用量。',
      );
    }
    final filtered = _filtered;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _searchController,
                decoration: const InputDecoration(
                  prefixIcon: Icon(Icons.search),
                  hintText: '搜索标题 / 目录 / 会话 ID',
                  border: OutlineInputBorder(),
                  isDense: true,
                ),
                onChanged: (value) => setState(() => _query = value),
              ),
            ),
            const SizedBox(width: 12),
            OutlinedButton.icon(
              onPressed: _exporting ? null : () => _export(true),
              icon: const Icon(Icons.data_object),
              label: const Text('导出 JSON'),
            ),
            const SizedBox(width: 8),
            OutlinedButton.icon(
              onPressed: _exporting ? null : () => _export(false),
              icon: const Icon(Icons.description_outlined),
              label: const Text('导出 MD'),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Expanded(
          child: filtered.isEmpty
              ? const EmptyState(
                  icon: Icons.search_off,
                  title: '没有匹配的会话',
                  message: '换个关键词试试。',
                )
              : ListView.separated(
                  padding: EdgeInsets.zero,
                  itemCount: filtered.length,
                  separatorBuilder: (_, _) => const SizedBox(height: 12),
                  itemBuilder: (context, index) {
                    final session = filtered[index];
                    final String title = session.title.isEmpty
                        ? session.sessionId
                        : session.title;
                    return GlassSurface(
                      onTap: () => _showDetail(session),
                      child: ListTile(
                        contentPadding: const EdgeInsets.symmetric(
                          horizontal: 16,
                          vertical: 6,
                        ),
                        leading: const Icon(
                          Icons.forum_outlined,
                          color: ferryAccent,
                        ),
                        title: Text(
                          title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                        subtitle: Text(
                          '${_fmtTokens(session.totalTokens)} tokens'
                          '（输入 ${_fmtTokens(session.inputTokens)} / 输出 ${_fmtTokens(session.outputTokens)}）'
                          '${session.updatedAt != null ? ' · ${_fmtSessionTime(session.updatedAt!)}' : ''}\n'
                          '${session.cwd}',
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: onSurface.withValues(alpha: 0.7),
                          ),
                        ),
                        isThreeLine: true,
                        trailing: Icon(
                          Icons.chevron_right,
                          color: onSurface.withValues(alpha: 0.4),
                        ),
                        onTap: () => _showDetail(session),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

/// 会话详情弹窗：异步加载 rollout 聊天记录并以气泡展示。
class _SessionDetailDialog extends StatefulWidget {
  const _SessionDetailDialog({required this.client, required this.session});

  final IpcClient client;
  final SessionRecord session;

  @override
  State<_SessionDetailDialog> createState() => _SessionDetailDialogState();
}

class _SessionDetailDialogState extends State<_SessionDetailDialog> {
  late Future<SessionDetail> _future;

  @override
  void initState() {
    super.initState();
    _future = widget.client.sessionDetail(widget.session.sessionId);
  }

  @override
  Widget build(BuildContext context) {
    final s = widget.session;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return AlertDialog(
      title: Text(
        s.title.isEmpty ? s.sessionId : s.title,
        maxLines: 2,
        overflow: TextOverflow.ellipsis,
      ),
      content: SizedBox(
        width: 560,
        height: 460,
        child: FutureBuilder<SessionDetail>(
          future: _future,
          builder: (context, snap) {
            if (snap.connectionState == ConnectionState.waiting) {
              return const Center(child: CircularProgressIndicator());
            }
            if (snap.hasError) {
              return Center(
                child: Text(
                  '加载会话失败：${snap.error}',
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              );
            }
            final detail = snap.requireData;
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  detail.cwd,
                  style: TextStyle(
                    color: onSurface.withValues(alpha: 0.7),
                    fontSize: 12.5,
                    fontFamily: 'monospace',
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  '${detail.totalTokens} tokens（输入 ${detail.inputTokens} / 输出 ${detail.outputTokens}）'
                  '${detail.updatedAt != null ? ' · ${_fmtSessionTime(detail.updatedAt!)}' : ''}',
                  style: TextStyle(
                    color: onSurface.withValues(alpha: 0.55),
                    fontSize: 12,
                  ),
                ),
                const Divider(height: 22),
                Expanded(
                  child: detail.messages.isEmpty
                      ? Center(
                          child: Text(
                            '（无聊天记录）',
                            style: TextStyle(
                              color: onSurface.withValues(alpha: 0.5),
                            ),
                          ),
                        )
                      : ListView.separated(
                          itemCount: detail.messages.length,
                          separatorBuilder: (_, _) =>
                              const SizedBox(height: 10),
                          itemBuilder: (context, i) =>
                              _ChatBubble(message: detail.messages[i]),
                        ),
                ),
              ],
            );
          },
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('关闭'),
        ),
      ],
    );
  }
}

/// 聊天气泡：用户右对齐、助手左对齐。
class _ChatBubble extends StatelessWidget {
  const _ChatBubble({required this.message});

  final ChatMessage message;

  @override
  Widget build(BuildContext context) {
    final bool user = message.isUser;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final Color bg = user
        ? ferryAccent.withValues(alpha: 0.14)
        : onSurface.withValues(alpha: 0.05);
    final Color border = user
        ? ferryAccent.withValues(alpha: 0.4)
        : onSurface.withValues(alpha: 0.1);
    return Align(
      alignment: user ? Alignment.centerRight : Alignment.centerLeft,
      child: Container(
        constraints: const BoxConstraints(maxWidth: 440),
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          color: bg,
          borderRadius: BorderRadius.circular(FerryRadii.control),
          border: Border.all(color: border),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              user ? '我' : 'Codex',
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w700,
                color: user ? ferryAccent : onSurface.withValues(alpha: 0.6),
              ),
            ),
            const SizedBox(height: 4),
            SelectableText(
              message.text,
              style: const TextStyle(fontSize: 13, height: 1.4),
            ),
          ],
        ),
      ),
    );
  }
}

/// 大数字紧凑显示（1.2k / 3.4m）。
String _fmtTokens(int v) {
  if (v >= 1000000000) return '${(v / 1e9).toStringAsFixed(1)}b';
  if (v >= 1000000) return '${(v / 1e6).toStringAsFixed(1)}m';
  if (v >= 1000) return '${(v / 1e3).toStringAsFixed(1)}k';
  return '$v';
}

/// 会话时间本地化短格式（YYYY-MM-DD HH:MM）。
String _fmtSessionTime(DateTime t) {
  final DateTime l = t.toLocal();
  String two(int n) => n.toString().padLeft(2, '0');
  return '${l.year}-${two(l.month)}-${two(l.day)} ${two(l.hour)}:${two(l.minute)}';
}

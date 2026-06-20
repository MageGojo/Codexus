/// 仪表盘统计数据（来自 `GET /ipc/stats`）。
class Stats {
  const Stats({
    required this.days,
    required this.totals,
    required this.series,
    required this.providerUsage,
  });

  final int days;
  final StatsTotals totals;
  final List<StatPoint> series;

  /// 按供应商/账号的 token 用量对比（上报 vs 本地估算，含掺假标记）。
  final List<ProviderUsage> providerUsage;

  factory Stats.fromJson(Map<String, dynamic> json) {
    return Stats(
      days: json['days'] as int? ?? 0,
      totals: StatsTotals.fromJson(
        json['totals'] as Map<String, dynamic>? ?? const {},
      ),
      series: (json['series'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(StatPoint.fromJson)
          .toList(),
      providerUsage: (json['provider_usage'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(ProviderUsage.fromJson)
          .toList(),
    );
  }

  static const Stats empty = Stats(
    days: 0,
    totals: StatsTotals.empty,
    series: [],
    providerUsage: [],
  );
}

/// 单个供应商/账号的 token 用量对比。
class ProviderUsage {
  const ProviderUsage({
    required this.provider,
    required this.isPool,
    required this.requests,
    required this.reportedTotal,
    required this.reportedInput,
    required this.reportedOutput,
    required this.estTotal,
    required this.estInput,
    required this.estOutput,
    required this.ratio,
    required this.suspect,
  });

  /// provider 标识（供应商 base_url 或 `codex-pool:<账号>`）。
  final String provider;
  final bool isPool;
  final int requests;
  final int reportedTotal;
  final int reportedInput;
  final int reportedOutput;
  final int estTotal;
  final int estInput;
  final int estOutput;

  /// 上报/估算 比值（estTotal>0 时有值）。
  final double? ratio;

  /// 疑似掺假（上游上报显著高于本地独立估算）。
  final bool suspect;

  /// 展示名：去掉协议前缀的 host，或账号池名。
  String get label {
    if (isPool) {
      return provider.replaceFirst('codex-pool:', '');
    }
    var s = provider.replaceFirst(RegExp(r'^https?://'), '');
    s = s.replaceFirst(RegExp(r'/v1/?$'), '');
    return s;
  }

  factory ProviderUsage.fromJson(Map<String, dynamic> json) {
    return ProviderUsage(
      provider: json['provider'] as String? ?? '',
      isPool: json['is_pool'] as bool? ?? false,
      requests: json['requests'] as int? ?? 0,
      reportedTotal: json['reported_total'] as int? ?? 0,
      reportedInput: json['reported_input'] as int? ?? 0,
      reportedOutput: json['reported_output'] as int? ?? 0,
      estTotal: json['est_total'] as int? ?? 0,
      estInput: json['est_input'] as int? ?? 0,
      estOutput: json['est_output'] as int? ?? 0,
      ratio: (json['ratio'] as num?)?.toDouble(),
      suspect: json['suspect'] as bool? ?? false,
    );
  }
}

class StatsTotals {
  const StatsTotals({
    required this.totalTokens,
    required this.inputTokens,
    required this.outputTokens,
    required this.requests,
    required this.succeeded,
    required this.failed,
    required this.successRate,
    required this.accountsCurrent,
    required this.accountsAdded,
    required this.accountsDeleted,
    required this.accountsExpired,
    required this.survivalRate,
  });

  final int totalTokens;
  final int inputTokens;
  final int outputTokens;
  final int requests;
  final int succeeded;
  final int failed;
  final double successRate;
  final int accountsCurrent;
  final int accountsAdded;
  final int accountsDeleted;
  final int accountsExpired;
  final double survivalRate;

  factory StatsTotals.fromJson(Map<String, dynamic> json) {
    return StatsTotals(
      totalTokens: json['total_tokens'] as int? ?? 0,
      inputTokens: json['input_tokens'] as int? ?? 0,
      outputTokens: json['output_tokens'] as int? ?? 0,
      requests: json['requests'] as int? ?? 0,
      succeeded: json['succeeded'] as int? ?? 0,
      failed: json['failed'] as int? ?? 0,
      successRate: (json['success_rate'] as num?)?.toDouble() ?? 0,
      accountsCurrent: json['accounts_current'] as int? ?? 0,
      accountsAdded: json['accounts_added'] as int? ?? 0,
      accountsDeleted: json['accounts_deleted'] as int? ?? 0,
      accountsExpired: json['accounts_expired'] as int? ?? 0,
      survivalRate: (json['survival_rate'] as num?)?.toDouble() ?? 0,
    );
  }

  static const StatsTotals empty = StatsTotals(
    totalTokens: 0,
    inputTokens: 0,
    outputTokens: 0,
    requests: 0,
    succeeded: 0,
    failed: 0,
    successRate: 0,
    accountsCurrent: 0,
    accountsAdded: 0,
    accountsDeleted: 0,
    accountsExpired: 0,
    survivalRate: 0,
  );
}

class StatPoint {
  const StatPoint({
    required this.date,
    required this.tokens,
    required this.inputTokens,
    required this.outputTokens,
    required this.requests,
    required this.succeeded,
    required this.failed,
    required this.accountsAdded,
    required this.accountsDeleted,
  });

  /// `YYYY-MM-DD`。
  final String date;
  final int tokens;
  final int inputTokens;
  final int outputTokens;
  final int requests;
  final int succeeded;
  final int failed;
  final int accountsAdded;
  final int accountsDeleted;

  /// `MM-DD` 短标签（图表横轴）。
  String get shortLabel => date.length >= 10 ? date.substring(5) : date;

  factory StatPoint.fromJson(Map<String, dynamic> json) {
    return StatPoint(
      date: json['date'] as String? ?? '',
      tokens: json['tokens'] as int? ?? 0,
      inputTokens: json['input_tokens'] as int? ?? 0,
      outputTokens: json['output_tokens'] as int? ?? 0,
      requests: json['requests'] as int? ?? 0,
      succeeded: json['succeeded'] as int? ?? 0,
      failed: json['failed'] as int? ?? 0,
      accountsAdded: json['accounts_added'] as int? ?? 0,
      accountsDeleted: json['accounts_deleted'] as int? ?? 0,
    );
  }
}

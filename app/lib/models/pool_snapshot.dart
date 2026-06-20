/// 账号池快照（对应后端 `/ipc/runtime/pool*` 返回的 `PoolResponse`）。
class PoolResponse {
  const PoolResponse({required this.mode, required this.snapshot});

  /// 当前路由模式（provider / pool）。
  final String mode;
  final PoolSnapshot snapshot;

  bool get poolActive => mode == 'pool';

  factory PoolResponse.fromJson(Map<String, dynamic> json) {
    return PoolResponse(
      mode: json['mode'] as String? ?? 'provider',
      snapshot: PoolSnapshot.fromJson(
        json['snapshot'] as Map<String, dynamic>? ?? const {},
      ),
    );
  }
}

class PoolSnapshot {
  const PoolSnapshot({
    this.total = 0,
    this.healthy = 0,
    this.coolingDown = 0,
    this.rotationEnabled = true,
    this.strategy = 'round_robin',
    this.pinned,
    this.current,
    this.accounts = const [],
  });

  final int total;
  final int healthy;
  final int coolingDown;
  final bool rotationEnabled;
  final String strategy;
  final String? pinned;
  final String? current;
  final List<PoolAccountStatus> accounts;

  bool get quotaAware => strategy == 'quota_aware';

  factory PoolSnapshot.fromJson(Map<String, dynamic> json) {
    return PoolSnapshot(
      total: json['total'] as int? ?? 0,
      healthy: json['healthy'] as int? ?? 0,
      coolingDown: json['cooling_down'] as int? ?? 0,
      rotationEnabled: json['rotation_enabled'] as bool? ?? true,
      strategy: json['strategy'] as String? ?? 'round_robin',
      pinned: json['pinned'] as String?,
      current: json['current'] as String?,
      accounts: (json['accounts'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(PoolAccountStatus.fromJson)
          .toList(),
    );
  }
}

class PoolAccountStatus {
  const PoolAccountStatus({
    required this.key,
    required this.displayName,
    this.accountId,
    this.authMode = '',
    this.healthy = true,
    this.coolingDown = false,
    this.cooldownRemainingSecs = 0,
    this.totalRequests = 0,
    this.totalFailures = 0,
    this.tokenPresent = true,
    this.lastError,
    this.isCurrent = false,
    this.planType,
    this.primaryUsedPercent,
    this.primaryResetAt,
    this.secondaryUsedPercent,
    this.secondaryResetAt,
    this.quotaUpdatedAt,
    this.quotaExhausted = false,
  });

  final String key;
  final String displayName;
  final String? accountId;
  final String authMode;
  final bool healthy;
  final bool coolingDown;
  final int cooldownRemainingSecs;
  final int totalRequests;
  final int totalFailures;
  final bool tokenPresent;
  final String? lastError;
  final bool isCurrent;
  final String? planType;
  final double? primaryUsedPercent;
  final int? primaryResetAt;
  final double? secondaryUsedPercent;
  final int? secondaryResetAt;
  final int? quotaUpdatedAt;
  final bool quotaExhausted;

  bool get hasQuota =>
      primaryUsedPercent != null ||
      secondaryUsedPercent != null ||
      (planType ?? '').isNotEmpty;

  factory PoolAccountStatus.fromJson(Map<String, dynamic> json) {
    double? d(Object? v) => v == null ? null : (v as num).toDouble();
    int? i(Object? v) => v == null ? null : (v as num).toInt();
    return PoolAccountStatus(
      key: json['key'] as String? ?? '',
      displayName: json['display_name'] as String? ?? '',
      accountId: json['account_id'] as String?,
      authMode: json['auth_mode'] as String? ?? '',
      healthy: json['healthy'] as bool? ?? true,
      coolingDown: json['cooling_down'] as bool? ?? false,
      cooldownRemainingSecs: json['cooldown_remaining_secs'] as int? ?? 0,
      totalRequests: json['total_requests'] as int? ?? 0,
      totalFailures: json['total_failures'] as int? ?? 0,
      tokenPresent: json['token_present'] as bool? ?? true,
      lastError: json['last_error'] as String?,
      isCurrent: json['is_current'] as bool? ?? false,
      planType: json['plan_type'] as String?,
      primaryUsedPercent: d(json['primary_used_percent']),
      primaryResetAt: i(json['primary_reset_at']),
      secondaryUsedPercent: d(json['secondary_used_percent']),
      secondaryResetAt: i(json['secondary_reset_at']),
      quotaUpdatedAt: i(json['quota_updated_at']),
      quotaExhausted: json['quota_exhausted'] as bool? ?? false,
    );
  }
}

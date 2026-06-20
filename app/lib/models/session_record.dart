/// Codex 会话记录（来自 `GET /ipc/sessions`，由后端 `ferry-codexlog` 解析
/// Codex CLI 本地 rollout 会话文件得到，换号后历史仍在）。
///
/// 字段为后端的 camelCase 序列化（`sessionId/title/cwd/updatedAt/...Tokens`），
/// `updatedAt` 为 Unix 秒。
class SessionRecord {
  const SessionRecord({
    required this.sessionId,
    required this.title,
    required this.cwd,
    required this.updatedAt,
    required this.inputTokens,
    required this.outputTokens,
    required this.totalTokens,
  });

  final String sessionId;
  final String title;
  final String cwd;

  /// 会话最近更新时间（rollout 文件修改时间）。
  final DateTime? updatedAt;
  final int inputTokens;
  final int outputTokens;
  final int totalTokens;

  factory SessionRecord.fromJson(Map<String, dynamic> json) {
    return SessionRecord(
      sessionId: json['sessionId'] as String? ?? '',
      title: json['title'] as String? ?? '',
      cwd: json['cwd'] as String? ?? '',
      updatedAt: epochSecondsToDate(json['updatedAt']),
      inputTokens: (json['inputTokens'] as num?)?.toInt() ?? 0,
      outputTokens: (json['outputTokens'] as num?)?.toInt() ?? 0,
      totalTokens: (json['totalTokens'] as num?)?.toInt() ?? 0,
    );
  }
}

/// 把后端的 Unix 秒（可能为 null）转为本地 [DateTime]。
DateTime? epochSecondsToDate(Object? value) {
  if (value is num) {
    return DateTime.fromMillisecondsSinceEpoch(value.toInt() * 1000);
  }
  return null;
}

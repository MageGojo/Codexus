import 'session_record.dart';

/// 会话中的一条消息（用户 / 助手），来自 rollout 解析。
class ChatMessage {
  const ChatMessage({required this.role, required this.text});

  final String role;
  final String text;

  bool get isUser => role == 'user';

  factory ChatMessage.fromJson(Map<String, dynamic> json) {
    return ChatMessage(
      role: json['role'] as String? ?? '',
      text: json['text'] as String? ?? '',
    );
  }
}

/// 单个会话的完整详情（含聊天记录），来自 `GET /ipc/sessions/{id}`。
class SessionDetail {
  const SessionDetail({
    required this.sessionId,
    required this.title,
    required this.cwd,
    required this.updatedAt,
    required this.inputTokens,
    required this.outputTokens,
    required this.totalTokens,
    required this.messages,
  });

  final String sessionId;
  final String title;
  final String cwd;
  final DateTime? updatedAt;
  final int inputTokens;
  final int outputTokens;
  final int totalTokens;
  final List<ChatMessage> messages;

  factory SessionDetail.fromJson(Map<String, dynamic> json) {
    return SessionDetail(
      sessionId: json['sessionId'] as String? ?? '',
      title: json['title'] as String? ?? '',
      cwd: json['cwd'] as String? ?? '',
      updatedAt: epochSecondsToDate(json['updatedAt']),
      inputTokens: (json['inputTokens'] as num?)?.toInt() ?? 0,
      outputTokens: (json['outputTokens'] as num?)?.toInt() ?? 0,
      totalTokens: (json['totalTokens'] as num?)?.toInt() ?? 0,
      messages: (json['messages'] as List<dynamic>? ?? const [])
          .whereType<Map<String, dynamic>>()
          .map(ChatMessage.fromJson)
          .toList(),
    );
  }
}

/// 古诗词（对应后端 `/ipc/integrations/poem`）。
class Poem {
  const Poem({
    required this.content,
    required this.origin,
    required this.author,
    this.category = '',
  });

  final String content;
  final String origin;
  final String author;
  final String category;

  bool get isEmpty => content.trim().isEmpty;

  /// 出处 + 作者，如「望天门山 · 李白」。
  String get attribution {
    final parts = <String>[
      if (origin.isNotEmpty) origin,
      if (author.isNotEmpty) author,
    ];
    return parts.join(' · ');
  }

  factory Poem.fromJson(Map<String, dynamic> json) {
    return Poem(
      content: json['content'] as String? ?? '',
      origin: json['origin'] as String? ?? '',
      author: json['author'] as String? ?? '',
      category: json['category'] as String? ?? '',
    );
  }
}

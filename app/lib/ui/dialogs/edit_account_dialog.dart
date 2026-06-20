import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/account_summary.dart';
import '../theme/app_theme.dart';
import '../widgets/glass_dialog.dart';

/// 打开「编辑账号」弹窗：自定义名称 / 标签 / 备注；API Key 账号可改 Key。
/// 保存成功返回 `true`，取消返回 `null`。
Future<bool?> showEditAccountDialog(
  BuildContext context,
  IpcClient client,
  AccountSummary account,
) {
  return showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _EditAccountDialog(client: client, account: account),
  );
}

class _EditAccountDialog extends StatefulWidget {
  const _EditAccountDialog({required this.client, required this.account});

  final IpcClient client;
  final AccountSummary account;

  @override
  State<_EditAccountDialog> createState() => _EditAccountDialogState();
}

class _EditAccountDialogState extends State<_EditAccountDialog> {
  late final TextEditingController _label;
  late final TextEditingController _tags;
  late final TextEditingController _note;
  final TextEditingController _apiKey = TextEditingController();
  bool _obscureKey = true;
  bool _busy = false;
  String? _error;

  // 模型（中转/厂商账号）：自动从供应商 /v1/models 拉取，回退内置目录。
  List<String> _models = const [];
  String _selectedModel = '';
  bool _fetchingModels = false;
  String? _modelHint;

  bool get _showModel => widget.account.vendorBound;

  @override
  void initState() {
    super.initState();
    final a = widget.account;
    _label = TextEditingController(text: a.label ?? '');
    _tags = TextEditingController(text: a.tags.join(', '));
    _note = TextEditingController(text: a.note ?? '');
    _selectedModel = a.model ?? '';
    if ((a.model ?? '').isNotEmpty) {
      _models = [a.model!];
    }
    if (_showModel) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _fetchModels());
    }
  }

  Future<void> _fetchModels() async {
    setState(() => _fetchingModels = true);
    try {
      final res = await widget.client.fetchProviderModels(widget.account.provider);
      if (!mounted) return;
      final merged = <String>[
        ...res.models,
        if (_selectedModel.isNotEmpty && !res.models.contains(_selectedModel))
          _selectedModel,
      ];
      setState(() {
        _models = merged;
        _modelHint = res.error ??
            (res.source == 'upstream' ? '已从供应商在线获取' : '使用内置模型目录');
      });
    } catch (e) {
      if (mounted) setState(() => _modelHint = '获取模型失败：$e');
    } finally {
      if (mounted) setState(() => _fetchingModels = false);
    }
  }

  @override
  void dispose() {
    _label.dispose();
    _tags.dispose();
    _note.dispose();
    _apiKey.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final tags = _tags.text
          .split(RegExp(r'[,，]'))
          .map((e) => e.trim())
          .where((e) => e.isNotEmpty)
          .toList();
      await widget.client.updateAccount(
        widget.account.id,
        label: _label.text.trim(),
        tags: tags,
        note: _note.text.trim(),
        apiKey: _apiKey.text.trim().isEmpty ? null : _apiKey.text.trim(),
        model: _showModel ? _selectedModel : null,
      );
      if (mounted) {
        Navigator.of(context).pop(true);
      }
    } catch (error) {
      if (mounted) {
        setState(() => _error = '$error');
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final AccountSummary a = widget.account;
    return GlassDialog(
      icon: Icons.edit_outlined,
      title: '编辑账号',
      subtitle: a.displayName,
      width: 520,
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: const Text('取消'),
        ),
        FilledButton(
          onPressed: _busy ? null : _submit,
          child: _busy
              ? const SizedBox.square(
                  dimension: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Text('保存'),
        ),
      ],
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (_error != null) ...[
            _ErrorBanner(message: _error!),
            const SizedBox(height: 14),
          ],
          _Field(
            label: '自定义名称',
            hint: '留空则用默认名（${a.displayName}）',
            controller: _label,
          ),
          const SizedBox(height: 14),
          _Field(
            label: '标签',
            hint: '逗号分隔，如：工作, 高优, 备用',
            controller: _tags,
          ),
          const SizedBox(height: 14),
          _Field(
            label: '备注',
            hint: '可选，给自己看的说明',
            controller: _note,
            maxLines: 3,
          ),
          if (_showModel) ...[
            const SizedBox(height: 14),
            _ModelPicker(
              models: _models,
              selected: _selectedModel,
              fetching: _fetchingModels,
              hint: _modelHint,
              providerName: a.provider,
              onChanged: (v) => setState(() => _selectedModel = v),
              onRefresh: (_busy || _fetchingModels) ? null : _fetchModels,
            ),
          ],
          if (a.canEditKey) ...[
            const SizedBox(height: 14),
            _Field(
              label: '更新 API Key',
              hint: '留空则不修改；填入新 sk-... 覆盖',
              controller: _apiKey,
              obscure: _obscureKey,
              trailing: IconButton(
                icon: Icon(
                  _obscureKey ? Icons.visibility_off : Icons.visibility,
                ),
                tooltip: _obscureKey ? '显示' : '隐藏',
                onPressed: () => setState(() => _obscureKey = !_obscureKey),
              ),
            ),
            const SizedBox(height: 8),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.info_outline,
                  size: 14,
                  color: onSurface.withValues(alpha: 0.5),
                ),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(
                    a.vendorBound
                        ? '更新后会同步给「${a.provider}」供应商，代理启用时自动取用。'
                        : '更新后保存到本地凭据文件（0600 权限）。',
                    style: TextStyle(
                      color: onSurface.withValues(alpha: 0.55),
                      fontSize: 12,
                      height: 1.4,
                    ),
                  ),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _Field extends StatelessWidget {
  const _Field({
    required this.label,
    required this.controller,
    this.hint,
    this.obscure = false,
    this.maxLines = 1,
    this.trailing,
  });

  final String label;
  final TextEditingController controller;
  final String? hint;
  final bool obscure;
  final int maxLines;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 2, bottom: 6),
          child: Text(
            label,
            style: TextStyle(
              color: onSurface.withValues(alpha: 0.75),
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
        TextField(
          controller: controller,
          obscureText: obscure,
          maxLines: obscure ? 1 : maxLines,
          decoration: InputDecoration(hintText: hint, suffixIcon: trailing),
        ),
      ],
    );
  }
}

/// 账号模型选择器：自动获取供应商模型 + 下拉选择 + 来源提示（液态玻璃风）。
class _ModelPicker extends StatelessWidget {
  const _ModelPicker({
    required this.models,
    required this.selected,
    required this.fetching,
    required this.hint,
    required this.providerName,
    required this.onChanged,
    required this.onRefresh,
  });

  final List<String> models;
  final String selected;
  final bool fetching;
  final String? hint;
  final String providerName;
  final ValueChanged<String> onChanged;
  final VoidCallback? onRefresh;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    // '' 默认项在前，模型按序，selected 必在列表内，去重。
    final seen = <String>{};
    final items = <String>[
      for (final m in <String>['', ...models, selected])
        if (seen.add(m)) m,
    ];
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 12, 14),
      decoration: BoxDecoration(
        color: ferryAccent.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(FerryRadii.control),
        border: Border.all(color: ferryAccent.withValues(alpha: 0.22)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.smart_toy_outlined, size: 16, color: ferryAccent),
              const SizedBox(width: 8),
              Text(
                '模型',
                style: TextStyle(
                  color: onSurface.withValues(alpha: 0.8),
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Spacer(),
              TextButton.icon(
                onPressed: onRefresh,
                icon: fetching
                    ? const SizedBox.square(
                        dimension: 14,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Icon(Icons.refresh, size: 16),
                label: Text(fetching ? '获取中' : '获取模型'),
              ),
            ],
          ),
          const SizedBox(height: 6),
          DropdownButtonFormField<String>(
            initialValue: items.contains(selected) ? selected : '',
            isExpanded: true,
            dropdownColor: ferryMenuColor(context),
            borderRadius: BorderRadius.circular(FerryRadii.control),
            icon: Icon(Icons.expand_more, color: onSurface.withValues(alpha: 0.6)),
            style: TextStyle(color: onSurface, fontSize: 13.5),
            decoration: const InputDecoration(
              isDense: true,
              border: OutlineInputBorder(),
              contentPadding: EdgeInsets.symmetric(horizontal: 12, vertical: 12),
            ),
            items: [
              for (final m in items)
                DropdownMenuItem(
                  value: m,
                  child: Text(
                    m.isEmpty ? '默认（用供应商默认模型）' : m,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
            ],
            onChanged: (v) => onChanged(v ?? ''),
          ),
          const SizedBox(height: 6),
          Text(
            hint ?? '点「获取模型」从「$providerName」自动拉取可选模型。使用该账号时即以此模型接管 Codex。',
            style: TextStyle(
              color: onSurface.withValues(alpha: 0.5),
              fontSize: 11.5,
              height: 1.4,
            ),
          ),
        ],
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    const Color danger = Color(0xFFF87171);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
      decoration: BoxDecoration(
        color: danger.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(FerryRadii.control),
        border: Border.all(color: danger.withValues(alpha: 0.4)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.error_outline, color: danger, size: 18),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              message,
              style: const TextStyle(color: danger, fontSize: 13, height: 1.4),
            ),
          ),
        ],
      ),
    );
  }
}

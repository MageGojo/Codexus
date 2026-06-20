import 'package:flutter/material.dart';

import '../../ipc/ipc_client.dart';
import '../../models/provider_preset.dart';
import '../theme/app_theme.dart';
import '../widgets/glass_dialog.dart';
import '../widgets/glass_segmented.dart';

/// 打开「添加 Codex 账号」弹窗。成功添加返回 `true`，取消返回 `null`。
///
/// [providers] 为供应商页配置的供应商（内置 + 自定义）；API Key 分页可选择
/// 把 Key 归属到其中某个供应商。
Future<bool?> showAddAccountDialog(
  BuildContext context,
  IpcClient client, {
  List<ProviderPreset> providers = const [],
  String? initialProviderId,
}) {
  return showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _AddAccountDialog(
      client: client,
      providers: providers,
      initialProviderId: initialProviderId,
    ),
  );
}

enum _AddTab { chatgpt, apiKey, json, importLocal }

class _AddAccountDialog extends StatefulWidget {
  const _AddAccountDialog({
    required this.client,
    this.providers = const [],
    this.initialProviderId,
  });

  final IpcClient client;
  final List<ProviderPreset> providers;

  /// 预选的供应商 id（从供应商页「添加账号」进入时带上）：
  /// 自动切到 API Key 分页并选中该供应商。
  final String? initialProviderId;

  @override
  State<_AddAccountDialog> createState() => _AddAccountDialogState();
}

class _AddAccountDialogState extends State<_AddAccountDialog> {
  _AddTab _tab = _AddTab.chatgpt;
  bool _busy = false;
  bool _obscureKey = true;
  String? _error;

  /// API Key 归属的供应商 id；null 表示通用 OpenAI 兼容账号（不绑定供应商）。
  String? _providerId;

  final TextEditingController _apiKey = TextEditingController();
  final TextEditingController _json = TextEditingController();

  @override
  void initState() {
    super.initState();
    final preset = widget.initialProviderId;
    if (preset != null && preset.isNotEmpty && preset != 'codex') {
      _tab = _AddTab.apiKey;
      _providerId = preset;
    }
  }

  @override
  void dispose() {
    _apiKey.dispose();
    _json.dispose();
    super.dispose();
  }

  String get _primaryLabel => switch (_tab) {
    _AddTab.chatgpt => '用 ChatGPT 登录',
    _AddTab.apiKey => '保存 API Key',
    _AddTab.json => '解析并添加',
    _AddTab.importLocal => '从本机导入',
  };

  Future<void> _submit() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      switch (_tab) {
        case _AddTab.chatgpt:
          await widget.client.loginWithChatGpt();
        case _AddTab.apiKey:
          final String key = _apiKey.text.trim();
          if (key.isEmpty) {
            _fail('请填写 API Key');
            return;
          }
          await widget.client.addApiKeyAccount(key, providerId: _providerId);
        case _AddTab.json:
          final String content = _json.text.trim();
          if (content.isEmpty) {
            _fail('请粘贴账号 JSON');
            return;
          }
          await widget.client.importCodexJson(content);
        case _AddTab.importLocal:
          await widget.client.importCodexLocal();
      }
      if (mounted) {
        Navigator.of(context).pop(true);
      }
    } catch (error) {
      _fail('$error');
    } finally {
      if (mounted) {
        setState(() => _busy = false);
      }
    }
  }

  void _fail(String message) {
    if (mounted) {
      setState(() => _error = message);
    }
  }

  ProviderPreset? _selectedProvider() {
    final id = _providerId;
    if (id == null) {
      return null;
    }
    for (final p in widget.providers) {
      if (p.id == id) {
        return p;
      }
    }
    return null;
  }

  void _switchTab(int index) {
    if (_busy) {
      return;
    }
    setState(() {
      _tab = _AddTab.values[index];
      _error = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return GlassDialog(
      icon: Icons.person_add_alt_1,
      title: '添加 Codex 账号',
      subtitle: '支持 ChatGPT 登录、API Key、Token 与本机导入',
      width: 560,
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
              : Text(_primaryLabel),
        ),
      ],
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          GlassSegmented(
            labels: const ['ChatGPT', 'API Key', 'JSON', '导入'],
            selectedIndex: _tab.index,
            onChanged: _switchTab,
          ),
          const SizedBox(height: 18),
          if (_error != null) ...[
            _ErrorBanner(message: _error!),
            const SizedBox(height: 14),
          ],
          _form(context),
        ],
      ),
    );
  }

  Widget _form(BuildContext context) {
    return switch (_tab) {
      _AddTab.chatgpt => _chatgptForm(context),
      _AddTab.apiKey => _apiKeyForm(context),
      _AddTab.json => _jsonForm(context),
      _AddTab.importLocal => _importForm(context),
    };
  }

  Widget _chatgptForm(BuildContext context) {
    return _InfoPanel(
      icon: Icons.open_in_browser,
      lines: [
        '点右下「用 ChatGPT 登录」，系统浏览器会打开 OpenAI 授权页。',
        '完成授权后自动跳回并保存账号（需本机 1455 端口空闲）。',
        if (_busy) '正在等待浏览器授权，请在浏览器中完成登录…',
      ],
    );
  }

  Widget _apiKeyForm(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final ProviderPreset? selected = _selectedProvider();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        _ProviderPicker(
          label: '归属供应商',
          providers: widget.providers,
          value: _providerId,
          onChanged: (value) => setState(() => _providerId = value),
        ),
        const SizedBox(height: 14),
        _Field(
          label: 'API Key',
          hint: selected == null
              ? '粘贴 sk-... 后保存到本地凭据文件（0600 权限）'
              : '粘贴 ${selected.name} 的 API Key（保存到本地凭据文件）',
          controller: _apiKey,
          obscure: _obscureKey,
          trailing: IconButton(
            icon: Icon(_obscureKey ? Icons.visibility_off : Icons.visibility),
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
                selected == null
                    ? '通用账号：保存为独立的 OpenAI 兼容 API Key 账号。'
                    : '将作为「${selected.name}」账号加入列表，并把该 Key 绑定给此供应商，'
                          '代理启用该供应商时自动取用。',
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
    );
  }

  Widget _jsonForm(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        _Field(
          label: '账号 JSON',
          hint:
              '粘贴 Codex 凭据 JSON，提交后由程序自动识别格式并解析…\n'
              '例如 auth.json 内容、{ "tokens": {...} }、或多账号数组 [ ... ]',
          controller: _json,
          maxLines: 9,
        ),
        const SizedBox(height: 8),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Icon(
              Icons.auto_awesome,
              size: 14,
              color: onSurface.withValues(alpha: 0.5),
            ),
            const SizedBox(width: 6),
            Expanded(
              child: Text(
                '自动识别：官方嵌套 / 扁平 / 裸 token 对象 / 数组 / { accounts: [...] }，'
                '无需手动拆分字段。',
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
    );
  }

  Widget _importForm(BuildContext context) {
    return _InfoPanel(
      icon: Icons.download_outlined,
      lines: const [
        '将读取本机 Codex 凭据并导入到 Codexus：',
      ],
      mono: '~/.codex/auth.json',
      footer: '同时支持 OAuth 与 API Key 两种本机凭据，导入后出现在下方列表。',
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
          decoration: InputDecoration(
            hintText: hint,
            suffixIcon: trailing,
          ),
        ),
      ],
    );
  }
}

class _ProviderPicker extends StatelessWidget {
  const _ProviderPicker({
    required this.label,
    required this.providers,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final List<ProviderPreset> providers;
  final String? value;
  final ValueChanged<String?> onChanged;

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
        DropdownButtonFormField<String?>(
          initialValue: value,
          isExpanded: true,
          dropdownColor: ferryMenuColor(context),
          borderRadius: BorderRadius.circular(FerryRadii.control),
          decoration: const InputDecoration(),
          items: [
            const DropdownMenuItem<String?>(
              value: null,
              child: Text('通用（OpenAI 兼容，独立账号）'),
            ),
            for (final p in providers)
              DropdownMenuItem<String?>(
                value: p.id,
                child: Text(
                  '${p.name} · ${p.builtin ? '内置' : '自定义'}',
                  overflow: TextOverflow.ellipsis,
                ),
              ),
          ],
          onChanged: onChanged,
        ),
      ],
    );
  }
}

class _InfoPanel extends StatelessWidget {
  const _InfoPanel({
    required this.icon,
    required this.lines,
    this.mono,
    this.footer,
  });

  final IconData icon;
  final List<String> lines;
  final String? mono;
  final String? footer;

  @override
  Widget build(BuildContext context) {
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final bool isDark = Theme.of(context).brightness == Brightness.dark;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: isDark ? const Color(0x0FFFFFFF) : const Color(0x0A1B2A4A),
        borderRadius: BorderRadius.circular(FerryRadii.control),
        border: Border.all(
          color: isDark ? const Color(0x1FFFFFFF) : const Color(0x14000000),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: ferryAccent, size: 20),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final String line in lines)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Text(
                      line,
                      style: TextStyle(
                        color: onSurface.withValues(alpha: 0.78),
                        height: 1.45,
                        fontSize: 13.5,
                      ),
                    ),
                  ),
                if (mono != null)
                  Container(
                    margin: const EdgeInsets.only(top: 2, bottom: 4),
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 6,
                    ),
                    decoration: BoxDecoration(
                      color: isDark
                          ? const Color(0x1438BDF8)
                          : const Color(0x1438BDF8),
                      borderRadius: BorderRadius.circular(FerryRadii.small),
                      border: Border.all(
                        color: ferryAccent.withValues(alpha: 0.3),
                      ),
                    ),
                    child: Text(
                      mono!,
                      style: const TextStyle(
                        fontFamily: 'monospace',
                        fontSize: 13,
                        color: ferryAccent,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                if (footer != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      footer!,
                      style: TextStyle(
                        color: onSurface.withValues(alpha: 0.55),
                        fontSize: 12.5,
                        height: 1.4,
                      ),
                    ),
                  ),
              ],
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

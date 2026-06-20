import 'package:flutter/material.dart';

import '../../models/account_summary.dart';
import '../../models/pool_snapshot.dart';
import '../theme/app_theme.dart';
import 'glass_surface.dart';
import 'quota_bar.dart';

/// 账号卡片（仿 cockpit-tools 账号网格）：头像 + 类型徽章 + 当前态高亮 +
/// 身份信息（存储方式 / account_id / 到期或更新时间）+ 操作 pill。
class AccountCard extends StatelessWidget {
  const AccountCard({
    super.key,
    required this.account,
    required this.onDelete,
    this.onUse,
    this.onEdit,
    this.busy = false,
    this.status,
    this.unavailable = false,
    this.unavailableReason,
  });

  final AccountSummary account;
  final VoidCallback onDelete;

  /// 「使用」此账号（切换运行时供应商 + 接管 Codex）。仅供应商绑定账号提供。
  final VoidCallback? onUse;

  /// 「编辑」此账号（自定义名称 / 标签 / 备注 / Key）。
  final VoidCallback? onEdit;
  final bool busy;

  /// 账号池配额/健康快照（仅 ChatGPT 账号有；含 5h/7d 已用百分比与重置时间）。
  final PoolAccountStatus? status;

  /// 账号是否不可用（令牌失效 / 额度耗尽 / 冷却 / 过期）——仿 cockpit：不可用也展示，
  /// 仅标注状态与原因，方便用户一眼看出哪个号不能用。
  final bool unavailable;
  final String? unavailableReason;

  @override
  Widget build(BuildContext context) {
    final TextTheme text = Theme.of(context).textTheme;
    final Color onSurface = Theme.of(context).colorScheme.onSurface;
    final bool current = account.current;
    final bool oauth = account.isOAuth;
    const Color dangerColor = Color(0xFFF87171);
    // 归属到具体供应商的 API Key 账号（provider 非空且非通用 codex）。
    final bool vendorBound =
        account.provider.isNotEmpty && account.provider != 'codex';

    Widget card = GlassSurface(
      strong: current,
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              _Avatar(oauth: oauth),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      account.displayName,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: text.titleMedium?.copyWith(
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      oauth
                          ? 'ChatGPT 登录'
                          : (vendorBound ? '供应商 API Key' : 'API Key'),
                      style: text.bodySmall?.copyWith(
                        color: onSurface.withValues(alpha: 0.55),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              if (unavailable)
                const _Chip(
                  label: '不可用',
                  color: dangerColor,
                  icon: Icons.error_outline,
                ),
              if (current)
                const _Chip(
                  label: '当前',
                  color: ferryAccent,
                  icon: Icons.check_circle,
                ),
              _Chip(
                label: oauth ? 'OAuth' : 'API',
                color: oauth ? const Color(0xFF8B5CF6) : ferryAccent,
              ),
              if (_planLabel(account.plan) != null)
                _Chip(
                  label: _planLabel(account.plan)!,
                  color: _planColor(account.plan),
                  icon: Icons.workspace_premium,
                ),
              if (vendorBound)
                const _Chip(
                  label: '供应商',
                  color: Color(0xFF38BDF8),
                  icon: Icons.hub,
                ),
              _Chip(
                label: account.storedInKeychain ? 'Keychain' : '文件',
                color: const Color(0xFF34D399),
                icon: account.storedInKeychain
                    ? Icons.lock_outline
                    : Icons.description_outlined,
              ),
            ],
          ),
          if (account.tags.isNotEmpty) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final tag in account.tags) _TagChip(label: tag),
              ],
            ),
          ],
          if (oauth)
            _AccountQuotaGrid(
              primaryPercent: status?.primaryUsedPercent,
              primaryResetAt: status?.primaryResetAt,
              secondaryPercent: status?.secondaryUsedPercent,
              secondaryResetAt: status?.secondaryResetAt,
              onSurface: onSurface,
            ),
          const SizedBox(height: 12),
          if (unavailable && (unavailableReason ?? '').isNotEmpty)
            _InfoRow(
              icon: Icons.warning_amber_rounded,
              label: '不可用 · ${unavailableReason!}',
              onSurface: onSurface,
              color: dangerColor,
            ),
          if ((account.model ?? '').isNotEmpty)
            _InfoRow(
              icon: Icons.smart_toy_outlined,
              label: '模型 ${account.model}',
              mono: true,
              onSurface: onSurface,
            ),
          _TokenUsageRow(account: account, onSurface: onSurface),
          if ((account.accountId ?? '').isNotEmpty)
            _InfoRow(
              icon: Icons.badge_outlined,
              label: account.accountId!,
              mono: true,
              onSurface: onSurface,
            ),
          if (account.expiresAt != null)
            _InfoRow(
              icon: Icons.schedule,
              label: '令牌到期 ${_fmt(account.expiresAt!)}',
              onSurface: onSurface,
            )
          else if (account.lastRefresh != null)
            _InfoRow(
              icon: Icons.update,
              label: '更新于 ${_fmt(account.lastRefresh!)}',
              onSurface: onSurface,
            ),
          const SizedBox(height: 14),
          Row(
            children: [
              if (onUse != null)
                _PillButton(
                  icon: Icons.play_arrow,
                  label: '使用',
                  color: const Color(0xFF34D399),
                  onTap: busy ? null : onUse,
                ),
              const Spacer(),
              if (onEdit != null) ...[
                _PillButton(
                  icon: Icons.edit_outlined,
                  label: '编辑',
                  color: ferryAccent,
                  onTap: busy ? null : onEdit,
                ),
                const SizedBox(width: 8),
              ],
              _PillButton(
                icon: Icons.delete_outline,
                label: '删除',
                color: const Color(0xFFF87171),
                onTap: busy ? null : onDelete,
              ),
            ],
          ),
        ],
      ),
    );

    if (current) {
      card = DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(FerryRadii.panel),
          border: Border.all(
            color: ferryAccent.withValues(alpha: 0.55),
            width: 1.5,
          ),
          boxShadow: [
            BoxShadow(
              color: ferryAccent.withValues(alpha: 0.22),
              blurRadius: 26,
              spreadRadius: -6,
              offset: const Offset(0, 10),
            ),
          ],
        ),
        child: card,
      );
    } else if (unavailable) {
      // 不可用账号：仍完整展示，仅加一圈淡红描边 + 整体略微变暗（仿 cockpit）。
      card = Opacity(
        opacity: 0.78,
        child: DecoratedBox(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(FerryRadii.panel),
            border: Border.all(
              color: dangerColor.withValues(alpha: 0.4),
              width: 1.2,
            ),
          ),
          child: card,
        ),
      );
    }
    return card;
  }

  static String _fmt(DateTime t) {
    final DateTime l = t.toLocal();
    String two(int n) => n.toString().padLeft(2, '0');
    return '${l.year}-${two(l.month)}-${two(l.day)} ${two(l.hour)}:${two(l.minute)}';
  }
}

class _Avatar extends StatelessWidget {
  const _Avatar({required this.oauth});

  final bool oauth;

  @override
  Widget build(BuildContext context) {
    final Color c = oauth ? const Color(0xFF8B5CF6) : ferryAccent;
    return Container(
      width: 42,
      height: 42,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(FerryRadii.control),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [c.withValues(alpha: 0.30), c.withValues(alpha: 0.10)],
        ),
        border: Border.all(color: c.withValues(alpha: 0.32)),
      ),
      child: Icon(
        oauth ? Icons.account_circle_outlined : Icons.vpn_key_outlined,
        color: c,
        size: 22,
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({required this.label, required this.color, this.icon});

  final String label;
  final Color color;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: EdgeInsets.fromLTRB(icon != null ? 6 : 8, 3, 8, 3),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: color.withValues(alpha: 0.45)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 12, color: color),
            const SizedBox(width: 3),
          ],
          Text(
            label,
            style: TextStyle(
              color: color,
              fontSize: 11,
              fontWeight: FontWeight.w700,
            ),
          ),
        ],
      ),
    );
  }
}

class _InfoRow extends StatelessWidget {
  const _InfoRow({
    required this.icon,
    required this.label,
    required this.onSurface,
    this.mono = false,
    this.color,
  });

  final IconData icon;
  final String label;
  final Color onSurface;
  final bool mono;

  /// 强调色（如不可用原因用红色）；为空时用默认中性色。
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final Color iconColor = color ?? onSurface.withValues(alpha: 0.45);
    final Color textColor = color ?? onSurface.withValues(alpha: 0.7);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Row(
        children: [
          Icon(icon, size: 14, color: iconColor),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: textColor,
                fontSize: 12.5,
                fontFamily: mono ? 'monospace' : null,
                fontWeight: color != null ? FontWeight.w600 : null,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// 标签胶囊。
class _TagChip extends StatelessWidget {
  const _TagChip({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    const Color c = Color(0xFFA78BFA);
    return Container(
      padding: const EdgeInsets.fromLTRB(6, 3, 8, 3),
      decoration: BoxDecoration(
        color: c.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(FerryRadii.small),
        border: Border.all(color: c.withValues(alpha: 0.42)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Icon(Icons.label_outline, size: 11, color: c),
          const SizedBox(width: 3),
          Text(
            label,
            style: const TextStyle(
              color: c,
              fontSize: 11,
              fontWeight: FontWeight.w600,
            ),
          ),
        ],
      ),
    );
  }
}

/// 账号 token 用量行：鼠标移入显示该账号用了多少 token（输入/输出/请求数）。
class _TokenUsageRow extends StatelessWidget {
  const _TokenUsageRow({required this.account, required this.onSurface});

  final AccountSummary account;
  final Color onSurface;

  @override
  Widget build(BuildContext context) {
    final bool has = account.tokensUsed > 0 || account.requests > 0;
    final String summary = has
        ? '已用 ${_compactTokens(account.tokensUsed)} tokens · ${account.requests} 次'
        : '暂无用量记录';
    final String tip = has
        ? '该账号累计用量\n'
              '输入 ${account.inputTokens} · 输出 ${account.outputTokens}\n'
              '总计 ${account.tokensUsed} tokens · ${account.requests} 次请求'
        : '该账号还没有经 Codexus 代理的成功请求';
    return Tooltip(
      message: tip,
      waitDuration: const Duration(milliseconds: 150),
      child: MouseRegion(
        cursor: SystemMouseCursors.help,
        child: Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: Row(
            children: [
              Icon(
                Icons.data_usage,
                size: 14,
                color: onSurface.withValues(alpha: 0.45),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  summary,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: onSurface.withValues(alpha: 0.7),
                    fontSize: 12.5,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// 账号卡片中部「额度网格」（仿 Cockpit Tools）：ChatGPT 账号显示官方
/// 5h / 7d 额度条 + 重置倒计时；无数据时提示去刷新。
class _AccountQuotaGrid extends StatelessWidget {
  const _AccountQuotaGrid({
    required this.primaryPercent,
    required this.primaryResetAt,
    required this.secondaryPercent,
    required this.secondaryResetAt,
    required this.onSurface,
  });

  final double? primaryPercent;
  final int? primaryResetAt;
  final double? secondaryPercent;
  final int? secondaryResetAt;
  final Color onSurface;

  @override
  Widget build(BuildContext context) {
    final bool hasData = primaryPercent != null || secondaryPercent != null;
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
      decoration: BoxDecoration(
        color: onSurface.withValues(alpha: 0.04),
        borderRadius: BorderRadius.circular(FerryRadii.control),
        border: Border.all(color: onSurface.withValues(alpha: 0.07)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(Icons.speed, size: 13, color: onSurface.withValues(alpha: 0.5)),
              const SizedBox(width: 6),
              Text(
                '额度',
                style: TextStyle(
                  color: onSurface.withValues(alpha: 0.6),
                  fontSize: 11.5,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          if (hasData)
            Row(
              children: [
                Expanded(
                  child: QuotaBar(
                    label: '5h',
                    usedPercent: primaryPercent,
                    resetAt: primaryResetAt,
                  ),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: QuotaBar(
                    label: '7d',
                    usedPercent: secondaryPercent,
                    resetAt: secondaryResetAt,
                  ),
                ),
              ],
            )
          else
            Text(
              '点上方「刷新额度」获取 5h / 7d 用量',
              style: TextStyle(
                color: onSurface.withValues(alpha: 0.4),
                fontSize: 11,
              ),
            ),
        ],
      ),
    );
  }
}

/// 把原始 `chatgpt_plan_type` 映射为友好 plan 徽章文案（plus→PLUS 等）；
/// 无法识别时取首段大写，空值返回 null（不显示徽章）。
String? _planLabel(String? plan) {
  final p = (plan ?? '').trim().toLowerCase();
  if (p.isEmpty) return null;
  if (p.contains('enterprise')) return 'ENTERPRISE';
  if (p.contains('team')) return 'TEAM';
  if (p.contains('business')) return 'BUSINESS';
  if (p.contains('edu')) return 'EDU';
  if (p.contains('pro')) return 'PRO';
  if (p.contains('plus')) return 'PLUS';
  if (p.contains('free')) return 'FREE';
  return p.split(RegExp(r'[_\s-]')).first.toUpperCase();
}

/// plan 徽章配色：付费档金色，免费/未知档灰色。
Color _planColor(String? plan) {
  final label = _planLabel(plan);
  if (label == null || label == 'FREE') {
    return const Color(0xFF94A3B8);
  }
  return const Color(0xFFFBBF24);
}

/// 大数字紧凑显示（1.2k / 3.4m）。
String _compactTokens(int v) {
  if (v >= 1000000000) return '${(v / 1e9).toStringAsFixed(1)}b';
  if (v >= 1000000) return '${(v / 1e6).toStringAsFixed(1)}m';
  if (v >= 1000) return '${(v / 1e3).toStringAsFixed(1)}k';
  return '$v';
}

class _PillButton extends StatelessWidget {
  const _PillButton({
    required this.icon,
    required this.label,
    required this.color,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final Color color;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final bool enabled = onTap != null;
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
          decoration: BoxDecoration(
            color: color.withValues(alpha: enabled ? 0.14 : 0.06),
            borderRadius: BorderRadius.circular(FerryRadii.control),
            border: Border.all(
              color: color.withValues(alpha: enabled ? 0.4 : 0.18),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                icon,
                size: 15,
                color: color.withValues(alpha: enabled ? 1 : 0.5),
              ),
              const SizedBox(width: 6),
              Text(
                label,
                style: TextStyle(
                  color: color.withValues(alpha: enabled ? 1 : 0.5),
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

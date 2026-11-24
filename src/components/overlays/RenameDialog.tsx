import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ArrowRight, Sparkles } from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  RenamePreviewItem,
  RenameRuleConfig,
} from '../../api/ipc';
import { resolveAddonMetadataTitle, stripLeadingBracketPrefixes } from '../../utils/format';
import { Button, Dialog, FilterChip, LinearProgress, TextField } from '../m3/primitives';

export interface SingleRenameDialogProps {
  addon: AddonSummary | null;
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onConfirm: (addonId: string, fileName: string) => Promise<void>;
}

export function SingleRenameDialog({
  addon,
  namePreference = 'workshop_title',
  busy,
  onClose,
  onConfirm,
}: SingleRenameDialogProps) {
  const { t } = useTranslation();
  const titlePref = namePreference === 'addon_title' ? 'addon_title' : 'workshop_title';
  const [name, setName] = useState(() =>
    addon
      ? (addon.currentFileName ?? `${resolveAddonMetadataTitle(addon, titlePref)}.vpk`)
      : '',
  );

  if (!addon) return null;

  const trimmed = name.trim();
  const cleanTitle = resolveAddonMetadataTitle(addon, titlePref);
  const hasExplicitTitle = Boolean(addon.workshopTitle?.trim() || addon.addonTitle?.trim());
  const baseTitle = hasExplicitTitle
    ? cleanTitle
    : stripLeadingBracketPrefixes(cleanTitle) || cleanTitle;

  return (
    <Dialog
      open={Boolean(addon)}
      title={t('dialogs.renameTitle')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || !trimmed}
            onClick={() => void onConfirm(addon.id, trimmed)}
          >
            {t('common.apply')}
          </Button>
        </>
      }
    >
      <TextField
        label={t('dialogs.newFileName')}
        value={name}
        onChange={(e) => setName(e.target.value)}
        autoFocus
      />
      <div className="l4a-row-wrap">
        <Button
          variant="tonal"
          size="sm"
          icon={<Sparkles size={14} />}
          onClick={() =>
            setName(addon.workshopId ? `[${addon.workshopId}] ${baseTitle}.vpk` : `${baseTitle}.vpk`)
          }
        >
          {t('dialogs.useTitle')}
        </Button>
      </div>
    </Dialog>
  );
}

export interface BatchRenameDialogProps {
  open: boolean;
  addonIds: string[];
  initialRules: RenameRuleConfig;
  busy: boolean;
  onPreview: (rules: RenameRuleConfig, addonIds: string[]) => Promise<RenamePreviewItem[]>;
  onConfirm: (rules: RenameRuleConfig, renames: [string, string][]) => Promise<void>;
  onClose: () => void;
}

export function BatchRenameDialog({
  open,
  addonIds,
  initialRules,
  busy,
  onPreview,
  onConfirm,
  onClose,
}: BatchRenameDialogProps) {
  const { t } = useTranslation();
  const [rules, setRules] = useState<RenameRuleConfig>(initialRules);
  const [previewState, setPreviewState] = useState<{
    reqKey: string;
    items: RenamePreviewItem[];
  } | null>(null);

  const reqKey = open ? `${JSON.stringify(rules)}|${addonIds.join(',')}` : '';

  useEffect(() => {
    if (!open) return;
    let active = true;
    void onPreview(rules, addonIds)
      .then((res) => {
        if (active) setPreviewState({ reqKey, items: res });
      })
      .catch(() => {
        if (active) setPreviewState({ reqKey, items: [] });
      });
    return () => {
      active = false;
    };
  }, [open, rules, addonIds, onPreview, reqKey]);

  const loading = open && previewState?.reqKey !== reqKey;
  const items = previewState?.reqKey === reqKey ? previewState.items : [];

  const validRenames: [string, string][] = items
    .filter((item) => !item.conflict && item.currentFileName !== item.targetFileName)
    .map((item) => [item.addonId, item.targetFileName]);

  return (
    <Dialog
      open={open}
      wide
      title={t('dialogs.batchRenameTitle')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || loading || validRenames.length === 0}
            onClick={() => void onConfirm(rules, validRenames)}
          >
            {t('common.apply')} ({validRenames.length})
          </Button>
        </>
      }
    >
      <div className="md-chip-row">
        <FilterChip
          label={t('settings.workshopIdPrefix')}
          selected={rules.enableWorkshopIdPrefix}
          onClick={() =>
            setRules((r) => ({ ...r, enableWorkshopIdPrefix: !r.enableWorkshopIdPrefix }))
          }
        />
        <FilterChip
          label={t('settings.groupPrefix')}
          selected={rules.enableGroupPrefix}
          onClick={() => setRules((r) => ({ ...r, enableGroupPrefix: !r.enableGroupPrefix }))}
        />
        <FilterChip
          label={t('settings.cleanSpecialChars')}
          selected={rules.cleanSpecialChars}
          onClick={() => setRules((r) => ({ ...r, cleanSpecialChars: !r.cleanSpecialChars }))}
        />
        <FilterChip
          label={t('settings.enableTrim')}
          selected={rules.enableTrim}
          onClick={() => setRules((r) => ({ ...r, enableTrim: !r.enableTrim }))}
        />
        <FilterChip
          label={t('settings.removeDoubleSpaces')}
          selected={rules.enableRemoveDoubleSpaces}
          onClick={() =>
            setRules((r) => ({ ...r, enableRemoveDoubleSpaces: !r.enableRemoveDoubleSpaces }))
          }
        />
      </div>

      {loading && <LinearProgress indeterminate label={t('dialogs.batchRenameTitle')} />}

      <div className="l4a-diff-table">
        {items.map((item) => {
          const unchanged = item.currentFileName === item.targetFileName;
          return (
            <div key={item.addonId} className="l4a-diff-row">
              <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
                <span className="md-body-small md-mono l4a-truncate" style={{ color: 'var(--md-sys-color-on-surface-variant)' }}>
                  {item.currentFileName}
                </span>
                {item.conflict ? (
                  <span className="md-pill" data-tone="error">
                    {t('dialogs.conflict')}
                  </span>
                ) : unchanged ? (
                  <span className="md-pill" data-tone="surface">
                    {t('dialogs.unchanged')}
                  </span>
                ) : null}
              </div>
              <div className="l4a-row-wrap" style={{ gap: 6 }}>
                <ArrowRight size={13} style={{ color: 'var(--md-sys-color-primary)', flexShrink: 0 }} />
                <span className="md-body-small md-mono l4a-truncate">{item.targetFileName}</span>
              </div>
              {item.conflict && (
                <span className="md-body-small" style={{ color: 'var(--md-sys-color-error)' }}>
                  {item.conflict}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}

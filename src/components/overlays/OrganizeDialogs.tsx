import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, ExternalLink, Search, Sparkles, X } from 'lucide-react';
import type {
  AddonNamePreference,
  AddonSummary,
  LoadoutProfile,
  UserGroup,
} from '../../api/ipc';
import { useContentFilterContext } from '../../state/ContentFilterContext';
import {
  getFilteredAddonFallbackCover,
  formatFilteredAddonTitle,
  formatRedactedAddonLabel,
  getAddonSearchRedaction,
  resolveLibraryAddonCardFilter,
} from '../../utils/contentFilter';
import {
  extractWorkshopIdInput,
  formatBytes,
  matchesAddonSearch,
} from '../../utils/format';
import { AddonCover } from '../m3/AddonCover';
import { FilterFailureBadge } from '../m3/ContentFilterIndicators';
import { Button, Checkbox, Dialog, FilterChip, IconButton, Switch, TextField } from '../m3/primitives';

const GROUP_COLORS = ['#246A4D', '#006399', '#964422', '#6750A4', '#8B5E00', '#AD2E56', '#3D6373'];

type PickerFilterMode = 'all' | 'selected' | 'manual';

function AddonPicker({
  addons,
  namePreference = 'workshop_title',
  missingIds,
  selected,
  onToggle,
  onSelectMany,
  onClearAll,
}: {
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  missingIds?: string[];
  selected: Set<string>;
  onToggle: (id: string) => void;
  onSelectMany?: (ids: string[]) => void;
  onClearAll?: () => void;
}) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  const [filter, setFilter] = useState('');
  const [mode, setMode] = useState<PickerFilterMode>('all');

  const nonHiddenAddons = useMemo(
    () => addons.filter((a) => !resolveLibraryAddonCardFilter(a, filterCtx).hiddenCard),
    [addons, filterCtx],
  );

  const visible = useMemo(() => {
    const q = filter.trim();
    return nonHiddenAddons.filter((a) => {
      if (mode === 'selected' && !selected.has(a.id)) return false;
      if (mode === 'manual' && a.primaryDir !== 'loading') return false;
      if (!q) return true;
      return matchesAddonSearch(a, q, getAddonSearchRedaction(a, filterCtx));
    });
  }, [nonHiddenAddons, filter, mode, selected, filterCtx]);

  const hasMixedDirs = useMemo(
    () =>
      nonHiddenAddons.some((a) => a.primaryDir === 'loading') &&
      nonHiddenAddons.some((a) => a.primaryDir === 'workshop'),
    [nonHiddenAddons],
  );

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div className="l4a-row-wrap" style={{ justifyContent: 'space-between' }}>
        <span className="md-field-label">
          {t('organize.pickAddons')} ({selected.size})
        </span>
        <div className="l4a-row-wrap" style={{ gap: 4 }}>
          {onSelectMany && visible.length > 0 && (
            <Button
              variant="text"
              size="sm"
              onClick={() => onSelectMany(visible.map((a) => a.id))}
            >
              {t('organize.pickerSelectVisible')}
            </Button>
          )}
          {onClearAll && selected.size > 0 && (
            <Button variant="text" size="sm" onClick={onClearAll}>
              {t('organize.pickerClear')}
            </Button>
          )}
        </div>
      </div>

      <div className="md-search-bar" style={{ height: 40, minWidth: 0 }}>
        <Search size={16} style={{ color: 'var(--md-sys-color-on-surface-variant)', flexShrink: 0 }} />
        <input
          className="md-search-input"
          style={{ fontSize: 13 }}
          placeholder={t('library.searchPlaceholder')}
          aria-label={t('common.search')}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        {filter && (
          <IconButton size="sm" label={t('common.clear')} onClick={() => setFilter('')}>
            <X size={14} />
          </IconButton>
        )}
      </div>

      <div className="md-chip-row">
        <FilterChip
          label={t('organize.pickerAll')}
          count={nonHiddenAddons.length}
          selected={mode === 'all'}
          onClick={() => setMode('all')}
        />
        <FilterChip
          label={t('organize.pickerSelected', { count: selected.size })}
          selected={mode === 'selected'}
          onClick={() => setMode((m) => (m === 'selected' ? 'all' : 'selected'))}
        />
        {hasMixedDirs && (
          <FilterChip
            label={t('organize.pickerManual')}
            selected={mode === 'manual'}
            onClick={() => setMode((m) => (m === 'manual' ? 'all' : 'manual'))}
          />
        )}
      </div>

      <div className="l4a-picker-list l4a-picker-list-tall">
        {missingIds?.map((id) => {
          const checked = selected.has(id);
          return (
            <div key={id} className="l4a-picker-row" onClick={() => onToggle(id)}>
              <Checkbox checked={checked} label={id} onChange={() => onToggle(id)} />
              <span className="md-body-medium md-mono l4a-truncate" style={{ flex: 1 }}>
                {id}
              </span>
              <span className="md-pill" data-tone="error">
                {t('organize.missingInProfile')}
              </span>
            </div>
          );
        })}
        {visible.length === 0 ? (
          <div style={{ padding: 24, textAlign: 'center', color: 'var(--md-sys-color-on-surface-variant)' }}>
            {t('common.noResults')}
          </div>
        ) : (
          visible.map((addon) => {
            const checked = selected.has(addon.id);
            const cardFilter = resolveLibraryAddonCardFilter(addon, filterCtx);
            const searchRedaction = getAddonSearchRedaction(addon, filterCtx);
            const displayName = formatFilteredAddonTitle(addon, namePreference, filterCtx, t);
            const authorText =
              searchRedaction.redactMeta || cardFilter.blurWholeCard
                ? t('filter.redactedAuthor')
                : addon.author || t('common.unknown');
            const { pendingEvaluation, failureDisplay, coverMode } = cardFilter;
            const evaluation = { pendingEvaluation, failureDisplay, coverMode };
            return (
              <div
                key={addon.id}
                className="l4a-picker-row"
                data-selected={checked ? 'true' : 'false'}
                onClick={() => onToggle(addon.id)}
              >
                <Checkbox
                  checked={checked}
                  label={displayName}
                  onChange={() => onToggle(addon.id)}
                />
                <div className="l4a-picker-thumb">
                  <AddonCover
                    compact
                    src={addon.coverImage}
                    fallbackSrc={getFilteredAddonFallbackCover(addon, filterCtx)}
                    title={displayName}
                    category={addon.categories[0]}
                    filterMode={coverMode}
                    evaluation={evaluation}
                  />
                </div>
                <div className="l4a-list-main">
                  <span className="md-title-small l4a-truncate">{displayName}</span>
                  <div className="l4a-addon-meta">
                    <span className="l4a-truncate">{authorText}</span>
                    <span>·</span>
                    <span>{formatBytes(addon.fileSize)}</span>
                    <FilterFailureBadge state={evaluation} />
                  </div>
                </div>
                {addon.primaryDir && (
                  <span className="md-pill" data-tone="surface">
                    {addon.primaryDir === 'loading'
                      ? t('library.loadingDir')
                      : t('library.workshopDir')}
                  </span>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

export function UserGroupDialog({
  open,
  initial,
  preselectedIds,
  addons,
  namePreference = 'workshop_title',
  busy,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: UserGroup | null;
  preselectedIds?: string[];
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onSave: (group: UserGroup) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(() => initial?.name ?? '');
  const [tagsRaw, setTagsRaw] = useState(() => initial?.tags.join(', ') ?? '');
  const [color, setColor] = useState(() => initial?.color ?? GROUP_COLORS[0]);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(
    () => new Set(initial ? initial.addonIds : (preselectedIds ?? [])),
  );

  const toggleId = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleAutoTags = () => {
    const counts = new Map<string, number>();
    for (const addon of addons) {
      if (!selectedIds.has(addon.id)) continue;
      for (const cat of addon.categories) {
        counts.set(cat, (counts.get(cat) ?? 0) + 1);
      }
    }
    const derived = Array.from(counts.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 5)
      .map(([cat]) => t(`categories.${cat}`, { defaultValue: cat }));
    if (derived.length > 0) {
      setTagsRaw(derived.join(', '));
    }
  };

  return (
    <Dialog
      open={open}
      wide
      title={initial ? t('common.edit') : t('organize.newGroup')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || !name.trim()}
            onClick={() => {
              const tags = tagsRaw
                .split(',')
                .map((s) => s.trim())
                .filter(Boolean);
              void onSave({
                id: initial?.id ?? `grp-${crypto.randomUUID?.() ?? Date.now()}`,
                name: name.trim(),
                tags,
                color,
                addonIds: Array.from(selectedIds),
              });
            }}
          >
            {t('common.save')}
          </Button>
        </>
      }
    >
      <div className="l4a-dialog-two-col">
        <TextField
          label={t('organize.groupName')}
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <TextField
          label={t('organize.groupTags')}
          value={tagsRaw}
          onChange={(e) => setTagsRaw(e.target.value)}
          trailing={
            <Button
              variant="text"
              size="sm"
              disabled={selectedIds.size === 0}
              icon={<Sparkles size={13} />}
              onClick={handleAutoTags}
            >
              {t('organize.autoTags')}
            </Button>
          }
        />
      </div>
      <div className="md-field">
        <span className="md-field-label">{t('organize.groupColor')}</span>
        <div className="l4a-color-swatches">
          {GROUP_COLORS.map((hex) => (
            <button
              key={hex}
              type="button"
              className="l4a-swatch"
              style={{ backgroundColor: hex }}
              data-selected={color === hex ? 'true' : 'false'}
              aria-label={hex}
              onClick={() => setColor(hex)}
            >
              {color === hex && <Check size={15} strokeWidth={2.8} />}
            </button>
          ))}
        </div>
      </div>
      <AddonPicker
        addons={addons}
        namePreference={namePreference}
        selected={selectedIds}
        onToggle={toggleId}
        onSelectMany={(ids) => setSelectedIds((prev) => new Set([...prev, ...ids]))}
        onClearAll={() => setSelectedIds(new Set())}
      />
    </Dialog>
  );
}

export function ManualBundleDialog({
  open,
  preselectedIds,
  addons,
  namePreference = 'workshop_title',
  busy,
  onClose,
  onCreate,
}: {
  open: boolean;
  preselectedIds?: string[];
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onCreate: (name: string, memberAddonIds: string[]) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState('');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(
    () => new Set(preselectedIds ?? []),
  );

  const toggleId = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <Dialog
      open={open}
      wide
      title={t('organize.newBundle')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || !name.trim() || selectedIds.size === 0}
            onClick={() => void onCreate(name.trim(), Array.from(selectedIds))}
          >
            {t('common.create')}
          </Button>
        </>
      }
    >
      <TextField
        label={t('organize.bundleName')}
        value={name}
        onChange={(e) => setName(e.target.value)}
        autoFocus
      />
      <AddonPicker
        addons={addons}
        namePreference={namePreference}
        selected={selectedIds}
        onToggle={toggleId}
        onSelectMany={(ids) => setSelectedIds((prev) => new Set([...prev, ...ids]))}
        onClearAll={() => setSelectedIds(new Set())}
      />
    </Dialog>
  );
}

export function TrackCollectionDialog({
  open,
  busy,
  onClose,
  onTrack,
}: {
  open: boolean;
  busy: boolean;
  onClose: () => void;
  onTrack: (collectionId: string, autoSync: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [raw, setRaw] = useState('');
  const [autoSync, setAutoSync] = useState(true);

  const parsedId = extractWorkshopIdInput(raw);

  return (
    <Dialog
      open={open}
      title={t('organize.newCollection')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || !parsedId}
            onClick={() => parsedId && void onTrack(parsedId, autoSync)}
          >
            {t('common.confirm')}
          </Button>
        </>
      }
    >
      <TextField
        label={t('organize.collectionInput')}
        value={raw}
        onChange={(e) => setRaw(e.target.value)}
        autoFocus
      />
      <div className="l4a-setting-row">
        <span className="md-body-medium">{t('organize.autoSync')}</span>
        <Switch checked={autoSync} label={t('organize.autoSync')} onChange={setAutoSync} />
      </div>
    </Dialog>
  );
}

export function ProfileDialog({
  open,
  initial,
  addons,
  namePreference = 'workshop_title',
  busy,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: LoadoutProfile | null;
  addons: AddonSummary[];
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onSave: (profile: LoadoutProfile) => Promise<void>;
}) {
  const { t } = useTranslation();
  const loadingAddons = useMemo(
    () => addons.filter((a) => a.installState === 'installed' && a.primaryDir === 'loading'),
    [addons],
  );
  const loadingIds = useMemo(() => new Set(loadingAddons.map((a) => a.id)), [loadingAddons]);

  const [name, setName] = useState(() => initial?.name ?? '');
  const [note, setNote] = useState(() => initial?.description ?? '');
  const [selectedIds, setSelectedIds] = useState<Set<string>>(() =>
    initial
      ? new Set(initial.enabledAddonIds)
      : new Set(loadingAddons.filter((a) => a.enabled).map((a) => a.id)),
  );

  const missingIds = useMemo(
    () => (initial?.enabledAddonIds ?? []).filter((id) => !loadingIds.has(id)),
    [initial, loadingIds],
  );

  const toggleId = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <Dialog
      open={open}
      wide
      title={initial ? t('common.edit') : t('organize.newProfile')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={busy || !name.trim()}
            onClick={() =>
              void onSave({
                id: initial?.id ?? `prof-${crypto.randomUUID?.() ?? Date.now()}`,
                name: name.trim(),
                description: note.trim(),
                enabledAddonIds: Array.from(selectedIds),
              })
            }
          >
            {t('common.save')}
          </Button>
        </>
      }
    >
      <div className="l4a-dialog-two-col">
        <TextField
          label={t('organize.profileName')}
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <TextField
          label={t('organize.profileNote')}
          value={note}
          onChange={(e) => setNote(e.target.value)}
        />
      </div>
      <div className="l4a-row-wrap">
        <Button
          variant="tonal"
          size="sm"
          icon={<Sparkles size={14} />}
          onClick={() =>
            setSelectedIds(new Set(loadingAddons.filter((a) => a.enabled).map((a) => a.id)))
          }
        >
          {t('organize.captureEnabled')}
        </Button>
      </div>
      <AddonPicker
        addons={loadingAddons}
        namePreference={namePreference}
        missingIds={missingIds}
        selected={selectedIds}
        onToggle={toggleId}
        onSelectMany={(ids) => setSelectedIds((prev) => new Set([...prev, ...ids]))}
        onClearAll={() => setSelectedIds(new Set())}
      />
    </Dialog>
  );
}

export function AddToGroupDialog({
  addonIds,
  groups,
  busy,
  onClose,
  onAssign,
  onCreateNew,
}: {
  addonIds: string[];
  groups: UserGroup[];
  busy: boolean;
  onClose: () => void;
  onAssign: (group: UserGroup, addonIds: string[]) => Promise<void>;
  onCreateNew: (addonIds: string[]) => void;
}) {
  const { t } = useTranslation();

  if (addonIds.length === 0) return null;

  return (
    <Dialog
      open={addonIds.length > 0}
      title={t('library.addToGroup')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={() => onCreateNew(addonIds)}>
            {t('organize.newGroup')}
          </Button>
          <Button variant="text" onClick={onClose}>
            {t('common.close')}
          </Button>
        </>
      }
    >
      <div className="l4a-picker-list">
        {groups.length === 0 ? (
          <div style={{ padding: 20, textAlign: 'center', color: 'var(--md-sys-color-on-surface-variant)' }}>
            {t('common.empty')}
          </div>
        ) : (
          groups.map((group) => {
            const allIn = addonIds.every((id) => group.addonIds.includes(id));
            return (
              <button
                key={group.id}
                type="button"
                disabled={busy}
                className="l4a-picker-row"
                onClick={() => void onAssign(group, addonIds)}
              >
                <span
                  style={{
                    width: 10,
                    height: 10,
                    borderRadius: 999,
                    backgroundColor: group.color || 'var(--md-sys-color-primary)',
                    flexShrink: 0,
                  }}
                />
                <span className="md-body-medium l4a-truncate" style={{ flex: 1 }}>
                  {group.name}
                </span>
                <span className="md-pill" data-tone={allIn ? 'primary' : 'surface'}>
                  {group.addonIds.length}
                </span>
              </button>
            );
          })
        )}
      </div>
    </Dialog>
  );
}

export function WorkshopToggleDialog({
  target,
  namePreference = 'workshop_title',
  busy,
  onClose,
  onMoveThenToggle,
  onToggleDirectly,
}: {
  target: { addon: AddonSummary; enabled: boolean } | null;
  namePreference?: AddonNamePreference;
  busy: boolean;
  onClose: () => void;
  onMoveThenToggle: (addon: AddonSummary, enabled: boolean) => Promise<void>;
  onToggleDirectly: (addon: AddonSummary, enabled: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  const filterCtx = useContentFilterContext();
  if (!target) return null;

  const redaction = getAddonSearchRedaction(target.addon, filterCtx);
  const targetLabel =
    redaction.redactTitle || redaction.redactMeta
      ? formatRedactedAddonLabel(target.addon.workshopId, t)
      : target.addon.currentFileName ||
        formatFilteredAddonTitle(target.addon, namePreference, filterCtx, t);

  return (
    <Dialog
      open={Boolean(target)}
      title={t('dialogs.workshopToggleTitle')}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="outlined"
            disabled={busy}
            onClick={() => void onToggleDirectly(target.addon, target.enabled)}
          >
            {t('dialogs.toggleDirectly')}
          </Button>
          <Button
            variant="filled"
            disabled={busy}
            onClick={() => void onMoveThenToggle(target.addon, target.enabled)}
          >
            {t('dialogs.moveFirst')}
          </Button>
        </>
      }
    >
      <div className="l4a-path-chip">
        <span className="md-mono l4a-truncate">{targetLabel}</span>
      </div>
    </Dialog>
  );
}

export function ExternalLinkDialog({
  url,
  onOpen,
  onClose,
}: {
  url: string | null;
  onOpen: (url: string) => Promise<boolean>;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [opening, setOpening] = useState(false);

  if (!url) return null;

  return (
    <Dialog
      open={Boolean(url)}
      title={t('dialogs.externalLinkTitle')}
      icon={<ExternalLink size={20} style={{ color: 'var(--md-sys-color-primary)' }} />}
      onClose={onClose}
      actions={
        <>
          <Button variant="text" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="filled"
            disabled={opening}
            onClick={async () => {
              setOpening(true);
              try {
                const ok = await onOpen(url);
                if (ok) onClose();
              } finally {
                setOpening(false);
              }
            }}
          >
            {t('common.open')}
          </Button>
        </>
      }
    >
      <div className="l4a-preview-box selectable">{url}</div>
    </Dialog>
  );
}

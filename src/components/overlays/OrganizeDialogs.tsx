import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, ExternalLink, Sparkles } from 'lucide-react';
import type { AddonSummary, LoadoutProfile, UserGroup } from '../../api/ipc';
import { extractWorkshopIdInput } from '../../utils/format';
import { Button, Checkbox, Dialog, Switch, TextField } from '../m3/primitives';

const GROUP_COLORS = ['#246A4D', '#006399', '#964422', '#6750A4', '#8B5E00', '#AD2E56', '#3D6373'];

function AddonPicker({
  addons,
  missingIds,
  selected,
  onToggle,
}: {
  addons: AddonSummary[];
  missingIds?: string[];
  selected: Set<string>;
  onToggle: (id: string) => void;
}) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState('');

  const visible = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return addons;
    return addons.filter(
      (a) =>
        a.title.toLowerCase().includes(q) ||
        a.workshopId?.includes(q) ||
        a.currentFileName?.toLowerCase().includes(q),
    );
  }, [addons, filter]);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <TextField
        label={`${t('organize.pickAddons')} (${selected.size})`}
        placeholder={t('common.search')}
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
      />
      <div className="l4a-picker-list">
        {missingIds?.map((id) => {
          const checked = selected.has(id);
          return (
            <div key={id} className="l4a-picker-row">
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
        {visible.map((addon) => {
          const checked = selected.has(addon.id);
          return (
            <div key={addon.id} className="l4a-picker-row">
              <Checkbox
                checked={checked}
                label={addon.title}
                onChange={() => onToggle(addon.id)}
              />
              <span
                className="md-body-medium l4a-truncate"
                style={{ flex: 1 }}
                onClick={() => onToggle(addon.id)}
              >
                {addon.title}
              </span>
              {addon.primaryDir && (
                <span className="md-pill" data-tone="surface">
                  {addon.primaryDir}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function UserGroupDialog({
  open,
  initial,
  preselectedIds,
  addons,
  busy,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: UserGroup | null;
  preselectedIds?: string[];
  addons: AddonSummary[];
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

  return (
    <Dialog
      open={open}
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
      />
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
      <AddonPicker addons={addons} selected={selectedIds} onToggle={toggleId} />
    </Dialog>
  );
}

export function ManualBundleDialog({
  open,
  preselectedIds,
  addons,
  busy,
  onClose,
  onCreate,
}: {
  open: boolean;
  preselectedIds?: string[];
  addons: AddonSummary[];
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
      <AddonPicker addons={addons} selected={selectedIds} onToggle={toggleId} />
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
  busy,
  onClose,
  onSave,
}: {
  open: boolean;
  initial: LoadoutProfile | null;
  addons: AddonSummary[];
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
        missingIds={missingIds}
        selected={selectedIds}
        onToggle={toggleId}
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
  busy,
  onClose,
  onMoveThenToggle,
  onToggleDirectly,
}: {
  target: { addon: AddonSummary; enabled: boolean } | null;
  busy: boolean;
  onClose: () => void;
  onMoveThenToggle: (addon: AddonSummary, enabled: boolean) => Promise<void>;
  onToggleDirectly: (addon: AddonSummary, enabled: boolean) => Promise<void>;
}) {
  const { t } = useTranslation();
  if (!target) return null;

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
        <span className="md-mono l4a-truncate">
          {target.addon.currentFileName || target.addon.title}
        </span>
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

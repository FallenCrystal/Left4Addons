import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertCircle, RefreshCw } from 'lucide-react';
import type { AddonDirType, AddonSummary, LoadoutProfile, UserGroup } from './api/ipc';
import { NavigationRail, type NavDestination } from './components/layout/NavigationRail';
import { TitleBar } from './components/layout/TitleBar';
import { Button, EmptyState, LoadingIndicator, Snackbar, TooltipHost } from './components/m3/primitives';
import { AddonDetailSheet } from './components/overlays/AddonDetailSheet';
import { BatchReportDialog } from './components/overlays/BatchReportDialog';
import { DeleteDialog } from './components/overlays/DeleteDialog';
import {
  AddToGroupDialog,
  ExternalLinkDialog,
  ManualBundleDialog,
  ProfileDialog,
  TrackCollectionDialog,
  UserGroupDialog,
  WorkshopToggleDialog,
} from './components/overlays/OrganizeDialogs';
import { BatchRenameDialog, SingleRenameDialog } from './components/overlays/RenameDialog';
import { TaskCenterSheet } from './components/overlays/TaskCenterSheet';
import { LibraryView, type ExternalAddonFilter } from './components/views/LibraryView';
import { OrganizeView } from './components/views/OrganizeView';
import { ReconcileView } from './components/views/ReconcileView';
import { SettingsView, type SettingsSection } from './components/views/SettingsView';
import { WorkshopView } from './components/views/WorkshopView';
import { setAppLanguage } from './i18n';
import { ContentFilterContext } from './state/ContentFilterContext';
import { useAppController } from './state/useAppController';
import { summarizeBatchReport } from './utils/format';
import {
  applyThemePreference,
  loadThemePreference,
  resolveColorScheme,
  type ThemePreference,
} from './utils/theme';

export function App() {
  const { t, i18n } = useTranslation();
  const [destination, setDestination] = useState<NavDestination>('library');
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('paths');
  const [themePref, setThemePref] = useState<ThemePreference>(() => loadThemePreference());
  const [systemScheme, setSystemScheme] = useState<'light' | 'dark'>(() =>
    resolveColorScheme('system'),
  );
  const resolvedTheme: 'light' | 'dark' =
    themePref.mode === 'system' ? systemScheme : themePref.mode;

  const [externalFilter, setExternalFilter] = useState<ExternalAddonFilter | null>(null);
  const [detailAddonId, setDetailAddonId] = useState<string | null>(null);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [tasksTab, setTasksTab] = useState<'queue' | 'sync'>('queue');

  const [singleRenameAddon, setSingleRenameAddon] = useState<AddonSummary | null>(null);
  const [batchRenameIds, setBatchRenameIds] = useState<string[] | null>(null);
  const [deleteTargets, setDeleteTargets] = useState<AddonSummary[]>([]);
  const [addToGroupIds, setAddToGroupIds] = useState<string[]>([]);
  const [bundleDialogIds, setBundleDialogIds] = useState<string[] | null>(null);
  const [groupDialogState, setGroupDialogState] = useState<{
    open: boolean;
    initial: UserGroup | null;
    preselectedIds?: string[];
  }>({ open: false, initial: null });
  const [profileDialogState, setProfileDialogState] = useState<{
    open: boolean;
    initial: LoadoutProfile | null;
  }>({ open: false, initial: null });
  const [trackDialogOpen, setTrackDialogOpen] = useState(false);
  const [workshopToggleTarget, setWorkshopToggleTarget] = useState<{
    addon: AddonSummary;
    enabled: boolean;
  } | null>(null);
  const [externalUrl, setExternalUrl] = useState<string | null>(null);

  const openReconcile = useCallback(() => setDestination('reconcile'), []);
  const ctrl = useAppController(openReconcile);

  const handleChangeTheme = useCallback((next: ThemePreference) => {
    setThemePref(next);
    if (next.mode === 'system') {
      setSystemScheme(resolveColorScheme('system'));
    }
  }, []);

  useEffect(() => {
    applyThemePreference(themePref);
    if (themePref.mode !== 'system' || typeof window.matchMedia !== 'function') {
      return;
    }
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const listener = () => {
      const next = applyThemePreference(themePref);
      setSystemScheme(next);
    };
    mq.addEventListener('change', listener);
    return () => mq.removeEventListener('change', listener);
  }, [themePref]);

  const addons = useMemo(() => ctrl.snapshot?.addons ?? [], [ctrl.snapshot]);
  const addonsById = useMemo(() => {
    const map = new Map<string, AddonSummary>();
    for (const a of addons) map.set(a.id, a);
    return map;
  }, [addons]);

  const activeDetailAddon = detailAddonId ? (addonsById.get(detailAddonId) ?? null) : null;
  const language: 'zh' | 'en' = i18n.language?.startsWith('en') ? 'en' : 'zh';

  const handleCopy = useCallback(
    async (text: string) => {
      if (!navigator.clipboard?.writeText) {
        ctrl.notify(t('common.copyFailed'), { tone: 'error' });
        return;
      }
      try {
        await navigator.clipboard.writeText(text);
        ctrl.notify(t('common.copied'));
      } catch {
        ctrl.notify(t('common.copyFailed'), { tone: 'error' });
      }
    },
    [ctrl, t],
  );

  const handleToggleSingle = useCallback(
    (addon: AddonSummary, enabled: boolean) => {
      if (addon.primaryDir === 'workshop') {
        setWorkshopToggleTarget({ addon, enabled });
        return;
      }
      void ctrl.toggleAddons([addon.id], enabled, false);
    },
    [ctrl],
  );

  const handleMoveSingle = useCallback(
    (addon: AddonSummary) => {
      const target: AddonDirType = addon.primaryDir === 'loading' ? 'workshop' : 'loading';
      void ctrl.moveAddons([addon.id], target);
    },
    [ctrl],
  );

  const handleBatchToggle = useCallback(
    (addonIds: string[], enabled: boolean) => {
      void ctrl.toggleAddons(addonIds, enabled, false);
    },
    [ctrl],
  );

  const handleOpenInFileManager = useCallback(
    (path: string) => {
      void ctrl.openInFileManager(path);
    },
    [ctrl],
  );

  const handleSyncAndNotify = useCallback(async () => {
    const report = await ctrl.sync();
    if (!report) return;
    const hasFindings =
      report.updatesAvailable.length > 0 ||
      report.removedUpstream.length > 0 ||
      report.dependencyDiffs.length > 0 ||
      report.obsoleteDependencies.length > 0 ||
      report.collectionDiffs.length > 0 ||
      report.warnings.length > 0;

    ctrl.notify(t('toast.syncDone', { count: report.checkedItems }), {
      actionLabel: hasFindings ? t('common.view') : undefined,
      onAction: hasFindings
        ? () => {
            setTasksTab('sync');
            setTasksOpen(true);
          }
        : undefined,
    });
  }, [ctrl, t]);

  const runningTasksCount = useMemo(() => {
    const liveEntries = Object.values(ctrl.liveDownloads);
    const liveTaskIds = new Set(liveEntries.map((d) => d.taskId));
    const liveWids = new Set(liveEntries.map((d) => d.workshopId));
    const persistedActive = (ctrl.snapshot?.tasks ?? []).filter(
      (task) =>
        (task.status === 'running' || task.status === 'queued') &&
        !liveTaskIds.has(task.id) &&
        !(task.kind === 'download' && liveWids.has(task.targetId)),
    ).length;
    return liveEntries.length + persistedActive;
  }, [ctrl.liveDownloads, ctrl.snapshot]);

  const openFilterSettings = useCallback(() => {
    setSettingsSection('filter');
    setDestination('settings');
  }, []);

  const filterContextValue = useMemo(
    () => ({
      filterConfig: ctrl.filterConfig,
      verdicts: ctrl.filterVerdicts,
      exclusionsByAddonId: ctrl.exclusionsByAddonId,
      revealedCardIds: ctrl.revealedCardIds,
      revealCard: ctrl.revealCard,
      registerWorkshopItems: ctrl.registerWorkshopItems,
      upsertFilterExclusion: ctrl.upsertFilterExclusion,
      deleteFilterExclusion: ctrl.deleteFilterExclusion,
      onOpenFilterSettings: openFilterSettings,
    }),
    [
      ctrl.deleteFilterExclusion,
      ctrl.exclusionsByAddonId,
      ctrl.filterConfig,
      ctrl.filterVerdicts,
      ctrl.registerWorkshopItems,
      ctrl.revealCard,
      ctrl.revealedCardIds,
      ctrl.upsertFilterExclusion,
      openFilterSettings,
    ],
  );

  const activityLabel = ctrl.syncing
    ? t('window.syncing')
    : ctrl.scanning
      ? t('window.scanning')
      : ctrl.loading && ctrl.snapshot
        ? t('window.loading')
        : null;

  return (
    <ContentFilterContext.Provider value={filterContextValue}>
      <div className="l4a-shell">
      <TitleBar activityLabel={activityLabel} isPreview={!ctrl.isTauri} />

      <NavigationRail
        active={destination}
        onSelect={setDestination}
        onScan={() => void ctrl.scan()}
        scanning={ctrl.scanning}
        openIssuesCount={ctrl.snapshot?.openIssues.length ?? 0}
        activeTasksCount={runningTasksCount}
        onOpenTasks={() => {
          setTasksTab('queue');
          setTasksOpen(true);
        }}
        resolvedTheme={resolvedTheme}
        onToggleTheme={() =>
          handleChangeTheme({
            ...themePref,
            mode: resolvedTheme === 'dark' ? 'light' : 'dark',
          })
        }
        language={language}
        onToggleLanguage={() => setAppLanguage(language === 'zh' ? 'en' : 'zh')}
        isPreview={!ctrl.isTauri}
      />

      <main className="l4a-workspace">
        {!ctrl.snapshot && ctrl.loading && !ctrl.initError && (
          <div className="l4a-loading-center">
            <LoadingIndicator size="lg" contained label={t('window.loading')} />
          </div>
        )}

        {!ctrl.snapshot && ctrl.initError && (
          <EmptyState
            icon={<AlertCircle size={28} style={{ color: 'var(--md-sys-color-error)' }} />}
            title={t('common.loadFailed')}
            action={
              <Button
                variant="filled"
                icon={<RefreshCw size={15} />}
                onClick={() => void ctrl.bootstrap()}
              >
                {t('common.retry')}
              </Button>
            }
          />
        )}

        {ctrl.snapshot && destination === 'library' && (
          <LibraryView
            addons={ctrl.snapshot.addons}
            bundles={ctrl.snapshot.bundles}
            namePreference={ctrl.snapshot.config.addonNamePreference ?? 'workshop_title'}
            collapseBundles={ctrl.snapshot.config.collapseCampaignBundles ?? true}
            onToggleCollapseBundles={(next) => {
              if (!ctrl.snapshot) return;
              void ctrl.saveConfig(
                { ...ctrl.snapshot.config, collapseCampaignBundles: next },
                false,
                true,
              );
            }}
            snapshotVersion={ctrl.snapshotVersion}
            liveDownloads={ctrl.liveDownloads}
            externalFilter={externalFilter}
            onClearExternalFilter={() => setExternalFilter(null)}
            busy={ctrl.busy}
            syncing={ctrl.syncing}
            onSync={() => void handleSyncAndNotify()}
            onScan={() => void ctrl.scan()}
            onSearchMapCode={ctrl.searchByMapCode}
            onSelectAddon={(addon) => setDetailAddonId(addon.id)}
            onToggleSingle={handleToggleSingle}
            onBatchToggle={handleBatchToggle}
            onBatchMove={(ids, dir) => void ctrl.moveAddons(ids, dir)}
            onBatchRename={(ids) => setBatchRenameIds(ids)}
            onBatchAddToGroup={(ids) => setAddToGroupIds(ids)}
            onBatchCreateBundle={(ids) => setBundleDialogIds(ids)}
            onBatchDelete={(items) => setDeleteTargets(items)}
            onDownloadItem={(wid, title) => void ctrl.downloadWorkshopItem(wid, title)}
            onUpdateItem={(wid, title) => void ctrl.updateWorkshopItem(wid, title)}
            onOpenInFileManager={handleOpenInFileManager}
            onCopy={(text) => void handleCopy(text)}
          />
        )}

        {ctrl.snapshot && destination === 'organize' && (
          <OrganizeView
            addons={ctrl.snapshot.addons}
            namePreference={ctrl.snapshot.config.addonNamePreference ?? 'workshop_title'}
            bundles={ctrl.snapshot.bundles}
            collections={ctrl.snapshot.collections}
            userGroups={ctrl.snapshot.userGroups}
            profiles={ctrl.snapshot.profiles}
            busy={ctrl.busy}
            onRebuildBundles={() => void ctrl.rebuildBundles()}
            onCreateBundle={() => setBundleDialogIds([])}
            onDeleteBundle={(id) => void ctrl.deleteBundle(id)}
            onToggleBundleLock={(id, locked) => void ctrl.setBundleLocked(id, locked)}
            onTrackCollection={() => setTrackDialogOpen(true)}
            onUntrackCollection={(id) => void ctrl.untrackCollection(id)}
            onDownloadWorkshopItem={(wid, title) => void ctrl.downloadWorkshopItem(wid, title)}
            onCreateGroup={() => setGroupDialogState({ open: true, initial: null })}
            onEditGroup={(group) => setGroupDialogState({ open: true, initial: group })}
            onSaveUserGroup={ctrl.saveUserGroup}
            onDeleteGroup={(id) => void ctrl.deleteUserGroup(id)}
            onCreateProfile={() => setProfileDialogState({ open: true, initial: null })}
            onEditProfile={(profile) => setProfileDialogState({ open: true, initial: profile })}
            onDeleteProfile={(id) => void ctrl.deleteProfile(id)}
            onApplyProfile={(id) => void ctrl.applyProfile(id)}
            onBulkToggle={handleBatchToggle}
            onToggleSingle={handleToggleSingle}
            onSelectAddon={(addon) => setDetailAddonId(addon.id)}
            onOpenInFileManager={handleOpenInFileManager}
            onOpenFilterInLibrary={(filter) => {
              setExternalFilter(filter);
              setDestination('library');
            }}
            onCopy={(text) => void handleCopy(text)}
          />
        )}

        {ctrl.snapshot && destination === 'reconcile' && (
          <ReconcileView
            issues={ctrl.snapshot.openIssues}
            addons={ctrl.snapshot.addons}
            config={ctrl.snapshot.config}
            busy={ctrl.busy}
            scanning={ctrl.scanning}
            onScan={() => void ctrl.scan()}
            onResolve={(id, action) => ctrl.resolveIssue(id, action)}
            onResolveBatch={ctrl.resolveIssuesBatch}
            onOpenInFileManager={handleOpenInFileManager}
            onOpenBypassSettings={() => {
              setSettingsSection('bypass');
              setDestination('settings');
            }}
          />
        )}

        {ctrl.snapshot && destination === 'workshop' && (
          <WorkshopView
            addons={ctrl.snapshot.addons}
            collections={ctrl.snapshot.collections}
            liveDownloads={ctrl.liveDownloads}
            onSearch={ctrl.searchWorkshop}
            onFetchItem={ctrl.fetchWorkshopItem}
            onImportSubscribed={ctrl.getSubscribedWorkshopIds}
            onDownload={ctrl.downloadWorkshopItem}
            onUpdate={ctrl.updateWorkshopItem}
            onCancelDownload={(wid) => void ctrl.cancelDownload(wid)}
            onTrackCollection={ctrl.trackCollection}
            onOpenSourcesSettings={() => {
              setSettingsSection('sources');
              setDestination('settings');
            }}
            onOpenExternal={(url) => setExternalUrl(url)}
            onInvalidId={() => ctrl.notify(t('toast.invalidWorkshopId'), { tone: 'error' })}
            onNotify={(msg) => ctrl.notify(msg)}
          />
        )}

        {ctrl.snapshot && destination === 'settings' && (
          <SettingsView
            config={ctrl.snapshot.config}
            section={settingsSection}
            onSectionChange={setSettingsSection}
            steamCaps={ctrl.steamCaps}
            checkingSteamCaps={ctrl.checkingSteamCaps}
            busy={ctrl.busy}
            themePref={themePref}
            language={language}
            addons={ctrl.snapshot.addons}
            exclusions={ctrl.snapshot.filterExclusions}
            localFilterStatus={ctrl.localFilterStatus}
            filterStatusBusy={ctrl.filterStatusBusy}
            filterTokenStatus={ctrl.filterTokenStatus}
            filterEvalError={ctrl.filterEvalError}
            onSaveConfig={ctrl.saveConfig}
            onDetectPath={ctrl.detectGamePath}
            onOpenInFileManager={handleOpenInFileManager}
            onRefreshSteamCaps={() => void ctrl.refreshSteamCapabilities()}
            onChangeTheme={handleChangeTheme}
            onChangeLanguage={setAppLanguage}
            onNotify={(msg, tone) => ctrl.notify(msg, { tone })}
            onRefreshLocalFilterStatus={ctrl.refreshLocalFilterStatus}
            onCheckLocalFilterUpdates={ctrl.checkLocalFilterUpdates}
            onDeployLocalFilter={ctrl.deployLocalFilter}
            onStartLocalFilterService={ctrl.startLocalFilterService}
            onStopLocalFilterService={ctrl.stopLocalFilterService}
            onRefreshFilterTokenStatus={ctrl.refreshFilterTokenStatus}
            onSetFilterLlmToken={ctrl.setFilterLlmToken}
            onClearFilterCache={ctrl.clearFilterCache}
            onEvaluateSandboxTarget={ctrl.evaluateSandboxTarget}
            onUpsertFilterExclusion={ctrl.upsertFilterExclusion}
            onDeleteFilterExclusion={ctrl.deleteFilterExclusion}
            onSelectAddon={(addon) => setDetailAddonId(addon.id)}
          />
        )}
      </main>

      <AddonDetailSheet
        addon={activeDetailAddon}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        bundles={ctrl.snapshot?.bundles ?? []}
        userGroups={ctrl.snapshot?.userGroups ?? []}
        busy={ctrl.busy}
        onClose={() => setDetailAddonId(null)}
        onToggle={handleToggleSingle}
        onMove={handleMoveSingle}
        onRename={(addon) => setSingleRenameAddon(addon)}
        onDelete={(addon) => setDeleteTargets([addon])}
        onDownload={(wid, title) => void ctrl.downloadWorkshopItem(wid, title)}
        onUpdate={(wid, title) => void ctrl.updateWorkshopItem(wid, title)}
        onAddToGroup={(addon) => setAddToGroupIds([addon.id])}
        onRemoveFromGroup={(group, addonId) => {
          void ctrl.saveUserGroup({
            ...group,
            addonIds: group.addonIds.filter((id) => id !== addonId),
          });
        }}
        onOpenExternal={(url) => setExternalUrl(url)}
        onOpenInFileManager={handleOpenInFileManager}
        onCopy={(text) => void handleCopy(text)}
      />

      <TaskCenterSheet
        open={tasksOpen}
        tab={tasksTab}
        onTabChange={setTasksTab}
        onClose={() => setTasksOpen(false)}
        addons={addons}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        tasks={ctrl.snapshot?.tasks ?? []}
        liveDownloads={ctrl.liveDownloads}
        syncReport={ctrl.syncReport}
        syncing={ctrl.syncing}
        busy={ctrl.busy}
        onSync={() => void handleSyncAndNotify()}
        onCancelDownload={(wid) => void ctrl.cancelDownload(wid)}
        onDownloadItem={(wid, title) => void ctrl.downloadWorkshopItem(wid, title)}
        onUpdateItem={(wid, title) => void ctrl.updateWorkshopItem(wid, title)}
        onSafeDeleteObsolete={(wid) => void ctrl.safeDeleteObsoleteDependency(wid, true, false)}
      />

      <SingleRenameDialog
        key={`single-rename:${singleRenameAddon?.id ?? 'closed'}`}
        addon={singleRenameAddon}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        busy={ctrl.busy}
        onClose={() => setSingleRenameAddon(null)}
        onConfirm={async (addonId, fileName) => {
          const report = await ctrl.applyRenames([[addonId, fileName]]);
          if (report && !summarizeBatchReport(report).hasIssue) {
            setSingleRenameAddon(null);
          }
        }}
      />

      {ctrl.snapshot && (
        <BatchRenameDialog
          key={`batch-rename:${batchRenameIds ? batchRenameIds.join(',') : 'closed'}`}
          open={batchRenameIds !== null}
          addonIds={batchRenameIds ?? []}
          initialRules={ctrl.snapshot.config.renameRules}
          busy={ctrl.busy}
          onPreview={ctrl.previewRename}
          onClose={() => setBatchRenameIds(null)}
          onConfirm={async (rules, renames) => {
            if (
              ctrl.snapshot &&
              JSON.stringify(rules) !== JSON.stringify(ctrl.snapshot.config.renameRules)
            ) {
              const saved = await ctrl.saveConfig(
                { ...ctrl.snapshot.config, renameRules: rules },
                false,
              );
              if (!saved) return;
            }
            const report = await ctrl.applyRenames(renames);
            if (report && !summarizeBatchReport(report).hasIssue) {
              setBatchRenameIds(null);
            }
          }}
        />
      )}

      <DeleteDialog
        key={`delete:${deleteTargets.map((item) => item.id).join(',') || 'closed'}`}
        addons={deleteTargets}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        busy={ctrl.busy}
        onClose={() => setDeleteTargets([])}
        onConfirm={async (ids, deleteDiskFile, purgeKnownRecord) => {
          const report = await ctrl.deleteAddons(ids, deleteDiskFile, purgeKnownRecord);
          if (report) {
            const failedIds = new Set(
              report.items
                .filter((i) => i.status === 'failed' || i.status === 'not_attempted')
                .map((i) => i.addonId),
            );
            if (
              detailAddonId &&
              ids.includes(detailAddonId) &&
              purgeKnownRecord &&
              !failedIds.has(detailAddonId)
            ) {
              setDetailAddonId(null);
            }
            if (!summarizeBatchReport(report).hasIssue) {
              setDeleteTargets([]);
            } else {
              setDeleteTargets((prev) => prev.filter((item) => failedIds.has(item.id)));
            }
          }
        }}
      />

      <BatchReportDialog
        payload={ctrl.batchReport}
        addonsById={addonsById}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        onClose={() => ctrl.setBatchReport(null)}
        onRescan={ctrl.scan}
      />

      <UserGroupDialog
        key={`user-group:${groupDialogState.open ? (groupDialogState.initial?.id ?? `new-${groupDialogState.preselectedIds?.join(',') ?? ''}`) : 'closed'}`}
        open={groupDialogState.open}
        initial={groupDialogState.initial}
        preselectedIds={groupDialogState.preselectedIds}
        addons={addons}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        busy={ctrl.busy}
        onClose={() => setGroupDialogState({ open: false, initial: null })}
        onSave={async (group) => {
          const ok = await ctrl.saveUserGroup(group);
          if (ok) setGroupDialogState({ open: false, initial: null });
        }}
      />

      <ManualBundleDialog
        key={`manual-bundle:${bundleDialogIds ? bundleDialogIds.join(',') || 'new' : 'closed'}`}
        open={bundleDialogIds !== null}
        preselectedIds={bundleDialogIds ?? undefined}
        addons={addons}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        busy={ctrl.busy}
        onClose={() => setBundleDialogIds(null)}
        onCreate={async (name, memberIds) => {
          const ok = await ctrl.createManualBundle(name, memberIds);
          if (ok) setBundleDialogIds(null);
        }}
      />

      <TrackCollectionDialog
        key={`track-collection:${trackDialogOpen ? 'open' : 'closed'}`}
        open={trackDialogOpen}
        busy={ctrl.busy}
        onClose={() => setTrackDialogOpen(false)}
        onTrack={async (collectionId, autoSync) => {
          const record = await ctrl.trackCollection(collectionId, autoSync);
          if (record) setTrackDialogOpen(false);
        }}
      />

      <ProfileDialog
        key={`profile:${profileDialogState.open ? (profileDialogState.initial?.id ?? 'new') : 'closed'}`}
        open={profileDialogState.open}
        initial={profileDialogState.initial}
        addons={addons}
        namePreference={ctrl.snapshot?.config.addonNamePreference ?? 'workshop_title'}
        busy={ctrl.busy}
        onClose={() => setProfileDialogState({ open: false, initial: null })}
        onSave={async (profile) => {
          const ok = await ctrl.saveProfile(profile);
          if (ok) setProfileDialogState({ open: false, initial: null });
        }}
      />

      <AddToGroupDialog
        addonIds={addToGroupIds}
        groups={ctrl.snapshot?.userGroups ?? []}
        busy={ctrl.busy}
        onClose={() => setAddToGroupIds([])}
        onCreateNew={(ids) => {
          setAddToGroupIds([]);
          setGroupDialogState({ open: true, initial: null, preselectedIds: ids });
        }}
        onAssign={async (group, ids) => {
          const merged = Array.from(new Set([...group.addonIds, ...ids]));
          const ok = await ctrl.saveUserGroup({ ...group, addonIds: merged });
          if (ok) setAddToGroupIds([]);
        }}
      />

      <WorkshopToggleDialog
        target={workshopToggleTarget}
        busy={ctrl.busy}
        onClose={() => setWorkshopToggleTarget(null)}
        onMoveThenToggle={async (addon, enabled) => {
          setWorkshopToggleTarget(null);
          const moved = await ctrl.moveAddons([addon.id], 'loading');
          const moveEntry = moved?.items.find((i) => i.addonId === addon.id);
          if (moveEntry && (moveEntry.status === 'applied' || moveEntry.status === 'unchanged')) {
            await ctrl.toggleAddons([addon.id], enabled, false);
          }
        }}
        onToggleDirectly={async (addon, enabled) => {
          setWorkshopToggleTarget(null);
          await ctrl.toggleAddons([addon.id], enabled, true);
        }}
      />

      <ExternalLinkDialog
        url={externalUrl}
        onOpen={ctrl.openExternalUrl}
        onClose={() => setExternalUrl(null)}
      />

        <Snackbar item={ctrl.snackbar} onClose={ctrl.dismissSnackbar} />
        <TooltipHost />
      </div>
    </ContentFilterContext.Provider>
  );
}

export default App;

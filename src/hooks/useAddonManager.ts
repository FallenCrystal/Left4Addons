import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Addon, Group, Settings, Toast, DatabasePayload, MasterCollection, WorkshopSourceSettings } from '../types/addon';
import { getAddonAuthor, getAddonCategories, getSuggestedVpkName, getAddonInfoValue, sortAddonsDownloadedFirst } from '../utils/addonHelpers';
import { useTranslation } from 'react-i18next';
import { useBackgroundTasks } from './useBackgroundTasks';
import type { BackgroundTask } from '../types/addon';
import type { WorkshopCapabilities } from '../components/workshop/types';
import { setSteamworksSdkDisabled, setWorkshopSourceSettings } from '../services/workshopClient';

export type RenderedItem = 
  | { type: 'group'; id: string; name: string; addons: Addon[]; groupObj: Group }
  | { type: 'addon'; id: string; data: Addon };

interface AddonsDbUpdatedEvent {
  data: DatabasePayload;
  external: boolean;
}

interface AddonsWatchErrorEvent {
  message: string;
}

interface DownloadProgressEvent {
  workshopId: string;
  percent: number;
}

export function useAddonManager() {
  const { t } = useTranslation();
  const [addons, setAddons] = useState<Record<string, Addon>>({});
  const [knownUninstalledAddons, setKnownUninstalledAddons] = useState<Record<string, Addon>>({});
  const [groups, setGroups] = useState<Group[]>([]);
  const [masterCollections, setMasterCollections] = useState<MasterCollection[]>([]);
  const [settings, setSettings] = useState<Settings>({
    workshopDir: '',
    loadingDir: '',
    downloadConcurrency: 2,
    enableDummyBypass: false,
    suppressSdkUnavailableWarning: false,
    disableSteamworksSdk: false,
    forceSteamworksSdkDownload: false,
    workshopSourceSettings: {
      preset: 'conservative',
      allowSteamworksSdk: true,
      allowSteamWebApi: true,
      allowSteamCommunityHtml: true,
      allowSdkHtmlHybrid: false,
      sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
      cacheRetention: 'keep',
    },
  });
  const [loading, setLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState('');
  const [currentFilterTab, setCurrentFilterTab] = useState('all'); // all, workshop, loading, disabled, groups, known-uninstalled, workshop-browser, master-collection
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [selectedMasterCollectionId, setSelectedMasterCollectionId] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState('All');
  const [sortBy, setSortBy] = useState('title'); // title, size, id
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [syncingSteam, setSyncingSteam] = useState(false);
  const [autoGrouping, setAutoGrouping] = useState(false);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [selectedGroupIds, setSelectedGroupIds] = useState<string[]>([]);
  const [isSelectMode, setIsSelectMode] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<Record<string, number>>({});
  const [workshopCapabilities, setWorkshopCapabilities] = useState<WorkshopCapabilities | null>(null);
  const [workshopCapabilitiesCheckedAt, setWorkshopCapabilitiesCheckedAt] = useState<string | null>(null);
  const [workshopCapabilitiesError, setWorkshopCapabilitiesError] = useState<string | null>(null);

  const clearDownloadProgress = useCallback((workshopId: string) => {
    setDownloadProgress((prev) => {
      if (prev[workshopId] === undefined) {
        return prev;
      }
      const next = { ...prev };
      delete next[workshopId];
      return next;
    });
  }, []);

  // Modals state
  const [detailModal, setDetailModal] = useState<{ open: boolean; addon: Addon | null }>({ open: false, addon: null });
  const [moveWarningModal, setMoveWarningModal] = useState<{ open: boolean; id: string; currentDirType: string; workshopId: string }>({ open: false, id: '', currentDirType: '', workshopId: '' });
  const [renameModal, setRenameModal] = useState<{ open: boolean; currentName: string; title: string; suggestedName: string }>({ open: false, currentName: '', title: '', suggestedName: '' });
  const [groupModal, setGroupModal] = useState<{ open: boolean }>({ open: false });
  const [editGroupModal, setEditGroupModal] = useState<{ open: boolean; groupId: string; name: string; tags: string[]; workshopCollectionId: string }>({ open: false, groupId: '', name: '', tags: [], workshopCollectionId: '' });
  const [settingsModal, setSettingsModal] = useState<{ open: boolean; loadingDir: string }>({ open: false, loadingDir: '' });
  const [linkConfirmModal, setLinkConfirmModal] = useState<{ open: boolean; url: string }>({ open: false, url: '' });
  const [confirmModal, setConfirmModal] = useState<{ open: boolean; title: string; message: string; onConfirm: (() => void) | null }>({ open: false, title: '', message: '', onConfirm: null });
  const [workshopActionModal, setWorkshopActionModal] = useState<{ open: boolean; actionName: string; addons: Addon[]; onProceed: (() => void) | null }>({ open: false, actionName: '', addons: [], onProceed: null });

  // Add toast helper
  const addToast = useCallback((message: string, type: 'success' | 'error' = 'success') => {
    const id = Math.random().toString(36).substring(2, 11);
    setToasts((prev) => [...prev, { id, message, type }]);
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4000);
  }, []);

  // Update local states from DatabasePayload
  const updateLocalState = useCallback((data: DatabasePayload) => {
    const nextAddons = data.addons || {};
    const nextKnownUninstalledAddons = data.knownUninstalledAddons || {};
    const nextGroups = data.groups || [];
    const nextMasterCollections = data.masterCollections || [];

    setAddons(nextAddons);
    setGroups(nextGroups);
    setKnownUninstalledAddons(nextKnownUninstalledAddons);
    setMasterCollections(nextMasterCollections);
    if (data.settings) {
      setSettings(data.settings);
    }

    setSelectedIds((prev) => prev.filter((id) => nextAddons[id] || nextKnownUninstalledAddons[id]));
    setSelectedGroupIds((prev) => prev.filter((id) => nextGroups.some((group) => group.id === id)));
    setDetailModal((prev) => {
      if (!prev.open || !prev.addon) {
        return prev;
      }

      const nextAddon = nextAddons[prev.addon.id] || nextKnownUninstalledAddons[prev.addon.id];
      return nextAddon
        ? { ...prev, addon: nextAddon }
        : { open: false, addon: null };
    });
  }, []);

  // Listen to backend events
  useEffect(() => {
    let unlistenDownload: (() => void) | undefined;
    let unlistenDbUpdate: (() => void) | undefined;
    let unlistenWatchError: (() => void) | undefined;

    const setupListeners = async () => {
      unlistenDownload = await listen<DownloadProgressEvent>('download-progress', (event) => {
        const { workshopId, percent } = event.payload;

        setDownloadProgress((prev) => ({
          ...prev,
          [workshopId]: percent,
        }));

        if (percent === 100) {
          setTimeout(() => {
            clearDownloadProgress(workshopId);
          }, 3000);
        }
      });

      unlistenDbUpdate = await listen<AddonsDbUpdatedEvent>('addons-db-updated', (event) => {
        updateLocalState(event.payload.data);
        if (event.payload.external) {
          addToast(t('toasts.autoRefreshSuccess'), 'success');
        }
      });

      unlistenWatchError = await listen<AddonsWatchErrorEvent>('addons-watch-error', (event) => {
        addToast(t('toasts.autoRefreshFailed', { err: event.payload.message }), 'error');
      });
    };

    setupListeners();

    return () => {
      if (unlistenDownload) unlistenDownload();
      if (unlistenDbUpdate) unlistenDbUpdate();
      if (unlistenWatchError) unlistenWatchError();
    };
  }, [addToast, clearDownloadProgress, t, updateLocalState]);

  const {
    backgroundTasks: managedBackgroundTasks,
    enqueueDownloads,
    enqueueWorkshopCrawl,
    recordSeenItems,
    cancelTask,
    retryTask,
    clearFinishedTasks,
  } = useBackgroundTasks({
    enabled: !loading,
    downloadConcurrency: settings.downloadConcurrency || 2,
    addons,
    knownUninstalledAddons,
    updateLocalState,
    onDownloadSuccess: () => {
      addToast(t('toasts.downloadSuccess'), 'success');
    },
    onDownloadCancelled: (workshopId) => {
      clearDownloadProgress(workshopId);
    },
    onTaskError: (message, workshopId) => {
      if (workshopId) {
        clearDownloadProgress(workshopId);
      }
      addToast(t('toasts.downloadFailed', { err: message }), 'error');
    },
  });

  const executeWithWorkshopCheck = async (addonsList: Addon[], actionName: string, proceed: () => void) => {
    const workshopAddons = addonsList.filter(ad => ad.dirType === 'workshop');
    const moveWorkshopAddonsAndProceed = async () => {
      try {
        const ids = workshopAddons.map(a => a.id);
        const data: DatabasePayload = await invoke('move_addons', { ids, targetDirType: 'loading' });
        updateLocalState(data);
      } catch (err) {
        addToast(t('toasts.autoMoveFailed', { err: String(err) }), 'error');
        setIsSubmitting(false);
        return;
      }
      proceed();
    };

    if (workshopAddons.length > 0) {
      if (sessionStorage.getItem('skipWorkshopWarning') !== 'true' && !settings.enableDummyBypass) {
        setWorkshopActionModal({
          open: true,
          actionName,
          addons: workshopAddons,
          onProceed: moveWorkshopAddonsAndProceed
        });
      } else {
        moveWorkshopAddonsAndProceed();
      }
    } else {
      proceed();
    }
  };

  // Fetch all data
  const fetchData = useCallback(async (showToastMessage = false) => {
    setLoading(true);
    try {
      const data: DatabasePayload = await invoke('get_addons');
      updateLocalState(data);
      if (showToastMessage) {
        addToast(t('toasts.dbRefreshSuccess'), 'success');
      }
    } catch (err) {
      console.error(err);
      addToast(t('toasts.dataLoadFailed', { err: String(err) }), 'error');
    } finally {
      setLoading(false);
    }
  }, [updateLocalState, addToast, t]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  useEffect(() => {
    setSteamworksSdkDisabled(settings.disableSteamworksSdk);
    setWorkshopSourceSettings(settings.workshopSourceSettings);
  }, [settings.disableSteamworksSdk, settings.workshopSourceSettings]);

  useEffect(() => {
    if (loading) {
      return;
    }

    const shouldProbeCapabilities = !settings.disableSteamworksSdk
      && (settings.workshopSourceSettings?.allowSteamworksSdk ?? true);

    if (!shouldProbeCapabilities) {
      setWorkshopCapabilities(null);
      setWorkshopCapabilitiesError(null);
      setWorkshopCapabilitiesCheckedAt(null);
      return;
    }

    let cancelled = false;

    void invoke<WorkshopCapabilities>('get_workshop_capabilities')
      .then((capabilities) => {
        if (cancelled) return;
        setWorkshopCapabilities(capabilities);
        setWorkshopCapabilitiesError(null);
        setWorkshopCapabilitiesCheckedAt(new Date().toISOString());
      })
      .catch((err) => {
        if (cancelled) return;
        setWorkshopCapabilities(null);
        setWorkshopCapabilitiesError(String(err));
        setWorkshopCapabilitiesCheckedAt(new Date().toISOString());
      });

    return () => {
      cancelled = true;
    };
  }, [loading, settings.disableSteamworksSdk, settings.workshopSourceSettings?.allowSteamworksSdk]);

  const steamworksUnavailableTask: BackgroundTask | null = (() => {
    if (loading || settings.suppressSdkUnavailableWarning || settings.disableSteamworksSdk) {
      return null;
    }
    const bridgeUnavailable = workshopCapabilities !== null && !workshopCapabilities.bridgeAvailable;
    if (!bridgeUnavailable && !workshopCapabilitiesError) {
      return null;
    }

    const detail = workshopCapabilities?.lastError || workshopCapabilitiesError || '';

    return {
      id: 'steamworks-sdk-unavailable',
      kind: 'warning',
      status: 'failed',
      source: 'frontend-warning',
      targetIds: [],
      progress: 100,
      createdAt: workshopCapabilitiesCheckedAt || new Date().toISOString(),
      title: t('taskCenter.steamworksUnavailableTitle'),
      error: detail || undefined,
    };
  })();

  const backgroundTasks = steamworksUnavailableTask
    ? [steamworksUnavailableTask, ...managedBackgroundTasks]
    : managedBackgroundTasks;

  const handleOpenLink = async (url: string) => {
    if (!url) return;
    if (url.startsWith('http://') || url.startsWith('https://')) {
      setLinkConfirmModal({ open: true, url });
    } else {
      try {
        await invoke('open_url', { url });
      } catch (err) {
        console.error('Failed to open link:', err);
      }
    }
  };

  // Toggle addon enable/disable status
  const toggleAddon = async (id: string, currentStatus: boolean) => {
    if (isSubmitting) return;
    const addon = addons[id] || knownUninstalledAddons[id];
    if (!addon) return;
    
    // If it's not installed, we can't toggle it (needs download)
    if (addon.dirType === 'none') {
      addToast('该组件未安装，请先下载安装', 'error');
      return;
    }

    setIsSubmitting(true);
    executeWithWorkshopCheck([addon], currentStatus ? '禁用组件' : '启用组件', async () => {
      try {
        const data: DatabasePayload = await invoke('toggle_addons', { ids: [id], enabled: !currentStatus });
        updateLocalState(data);
        addToast(currentStatus ? t('toasts.addonDisabled') : t('toasts.addonEnabled'), 'success');
        
        if (detailModal.open && detailModal.addon?.id === id) {
          setDetailModal(prev => ({
            ...prev,
            addon: prev.addon ? { ...prev.addon, isEnabled: !currentStatus } : null
          }));
        }
      } catch (err) {
        addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
      } finally {
        setIsSubmitting(false);
      }
    });
  };

  const toggleGroupAddons = async (addonsList: Addon[], enabled: boolean) => {
    if (isSubmitting) return;
    // Filter out uninstalled addons in the group
    const actionableAddons = addonsList.filter(ad => ad.dirType !== 'none' && ad.isEnabled !== enabled);
    if (actionableAddons.length === 0) return;
    
    setIsSubmitting(true);
    executeWithWorkshopCheck(actionableAddons, enabled ? '批量启用组件' : '批量禁用组件', async () => {
      try {
        const ids = actionableAddons.map(ad => ad.id);
        const data: DatabasePayload = await invoke('toggle_addons', { ids, enabled });
        updateLocalState(data);
        addToast(enabled ? t('toasts.groupEnabled') : t('toasts.groupDisabled'), 'success');
      } catch (err) {
        addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
      } finally {
        setIsSubmitting(false);
      }
    });
  };

  // Move addon to load or workshop directory
  const moveAddon = async (id: string, currentDirType: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    const targetDirType = currentDirType === 'workshop' ? 'loading' : 'workshop';
    try {
      const data: DatabasePayload = await invoke('move_addons', { ids: [id], targetDirType });
      updateLocalState(data);
      addToast(targetDirType === 'loading' ? t('toasts.moveSuccessLoading') : t('toasts.moveSuccessWorkshop'), 'success');
    } catch (err) {
      addToast(t('toasts.moveFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleMoveClick = (addon: Addon) => {
    if (addon.dirType === 'workshop') {
      if (settings.enableDummyBypass) {
        moveAddon(addon.id, addon.dirType);
      } else {
        setMoveWarningModal({
          open: true,
          id: addon.id,
          currentDirType: addon.dirType,
          workshopId: addon.workshopId || ''
        });
      }
    } else {
      moveAddon(addon.id, addon.dirType);
    }
  };

  // Sync Steam Details
  const syncSteamDetails = async () => {
    setSyncingSteam(true);
    try {
      const data: DatabasePayload = await invoke('steam_sync');
      updateLocalState(data);
      addToast(t('toasts.steamSyncSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.steamSyncFailed', { err: String(err) }), 'error');
    } finally {
      setSyncingSteam(false);
    }
  };

  // Run Auto Grouping
  const runAutoGrouping = async () => {
    setAutoGrouping(true);
    try {
      const data: DatabasePayload = await invoke('group_action', { action: 'auto-group' });
      updateLocalState(data);
      addToast(t('toasts.autoClassifySuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.autoClassifyFailed', { err: String(err) }), 'error');
    } finally {
      setAutoGrouping(false);
    }
  };

  // Save Settings
  const saveSettings = async (
    loadingDir: string,
    downloadConcurrency: number,
    enableDummyBypass: boolean,
    suppressSdkUnavailableWarning: boolean,
    disableSteamworksSdk: boolean,
    forceSteamworksSdkDownload: boolean,
    workshopSourceSettings?: WorkshopSourceSettings,
  ) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('save_settings', {
        payload: {
          loadingDir,
          downloadConcurrency,
          enableDummyBypass,
          suppressSdkUnavailableWarning,
          disableSteamworksSdk,
          forceSteamworksSdkDownload,
          workshopSourceSettings,
        },
      });
      updateLocalState(data);
      setSettingsModal({ open: false, loadingDir: '' });
      addToast(t('toasts.settingsSaveSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.settingsSaveFailed', { err: String(err) }), 'error');
      throw err;
    } finally {
      setIsSubmitting(false);
    }
  };

  // Save Rename Addon
  const submitRename = async (currentName: string, newVpkName: string) => {
    if (isSubmitting) return;
    const addon = addons[currentName] || knownUninstalledAddons[currentName];
    if (!addon) return;
    setIsSubmitting(true);
    executeWithWorkshopCheck([addon], t('common.rename'), async () => {
      try {
        const data: DatabasePayload = await invoke('rename_addon', {
          id: currentName,
          newVpkName
        });
        updateLocalState(data);
        setRenameModal({ open: false, currentName: '', title: '', suggestedName: '' });
        addToast(t('toasts.renameSuccess'), 'success');
      } catch (err) {
        addToast(t('toasts.renameFailed', { err: String(err) }), 'error');
      } finally {
        setIsSubmitting(false);
      }
    });
  };

  // Group Management APIs
  const handleCreateGroup = async (name: string, selectedAddons?: string[], tags?: string[], workshopCollectionId?: string, source?: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'create',
        name,
        ids: selectedAddons || [],
        tags,
        workshopCollectionId,
        source
      });
      updateLocalState(data);
      setGroupModal({ open: false });
      addToast(t('toasts.createGroupSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.createGroupFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleDeleteGroup = (groupId: string) => {
    setConfirmModal({
      open: true,
      title: t('confirmModal.deleteGroupTitle'),
      message: t('confirmModal.deleteGroupMsg'),
      onConfirm: async () => {
        if (isSubmitting) return;
        setIsSubmitting(true);
        try {
          const data: DatabasePayload = await invoke('group_action', { action: 'delete', groupId });
          updateLocalState(data);
          if (selectedGroupId === groupId) {
            setSelectedGroupId(null);
            setCurrentFilterTab('all');
          }
          addToast(t('toasts.deleteGroupSuccess'), 'success');
        } catch (err) {
          addToast(t('toasts.deleteGroupFailed', { err: String(err) }), 'error');
        } finally {
          setIsSubmitting(false);
        }
      }
    });
  };

  const handleRenameGroup = async (groupId: string, name: string, tags?: string[], workshopCollectionId?: string, ids?: string[]) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'update-group',
        groupId,
        name,
        tags,
        workshopCollectionId,
        ids
      });
      updateLocalState(data);
      setEditGroupModal({ open: false, groupId: '', name: '', tags: [], workshopCollectionId: '' });
      addToast(t('toasts.renameGroupSuccess') || '修改分组成功', 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const groupActionBatch = async (groupId: string, actionType: 'enable' | 'disable' | 'move-load') => {
    const group = groups.find(g => g.id === groupId);
    if (!group) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    
    try {
      if (actionType === 'enable' || actionType === 'disable') {
        const enabled = actionType === 'enable';
        const groupAddons = group.addons.map(name => addons[name] || knownUninstalledAddons[name]).filter(Boolean);
        const actionableAddons = groupAddons.filter(ad => ad.dirType !== 'none' && ad.isEnabled !== enabled);
        if (actionableAddons.length === 0) {
          setIsSubmitting(false);
          return;
        }
        executeWithWorkshopCheck(actionableAddons, enabled ? t('batchActionBar.enable') : t('batchActionBar.disable'), async () => {
          try {
            const ids = actionableAddons.map(ad => ad.id);
            const data: DatabasePayload = await invoke('toggle_addons', { ids, enabled });
            updateLocalState(data);
            addToast(enabled ? t('toasts.groupEnabled') : t('toasts.groupDisabled'), 'success');
          } catch (err) {
            addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
          } finally {
            setIsSubmitting(false);
          }
        });
      } else if (actionType === 'move-load') {
        const targetDirType = 'loading';
        const movableAddons = group.addons
          .map(name => addons[name])
          .filter((addon): addon is Addon => Boolean(addon))
          .filter(ad => ad.dirType === 'workshop');
        if (movableAddons.length === 0) {
          setIsSubmitting(false);
          return;
        }
        
        const executeMove = async () => {
          try {
            const ids = movableAddons.map(ad => ad.id);
            const data: DatabasePayload = await invoke('move_addons', { ids, targetDirType });
            updateLocalState(data);
            addToast(t('toasts.batchMoveSuccess'), 'success');
          } catch (err) {
            addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
          } finally {
            setIsSubmitting(false);
          }
        };

        if (settings.enableDummyBypass) {
          await executeMove();
        } else {
          setConfirmModal({
            open: true,
            title: t('moveWarningModal.title'),
            message: (t('confirmModal.batchMoveMsg', { count: movableAddons.length }) as unknown as string[]).join('\n\n'),
            onConfirm: executeMove
          });
        }
      }
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
      setIsSubmitting(false);
    }
  };

  const removeAddonFromGroup = async (id: string, groupId: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'remove-addons',
        groupId,
        ids: [id]
      });
      updateLocalState(data);
      addToast(t('toasts.removeFromGroupSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const addAddonToGroup = async (id: string, groupId: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        ids: [id]
      });
      updateLocalState(data);
      addToast(t('toasts.addToGroupSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: (err as Error).message }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  // Helper to suggest workshop title for rename
  const triggerRenameModal = (addon: Addon) => {
    const itemGroup = groups.find(g => g.addons.includes(addon.id));
    const suggestedVpkName = getSuggestedVpkName(addon, itemGroup?.name, addons);
    const steamTitle = addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName;

    setRenameModal({
      open: true,
      currentName: addon.id,
      suggestedName: suggestedVpkName,
      title: steamTitle
    });
  };

  // Download addon
  const downloadAddon = async (workshopId: string, title?: string, imagePath?: string) => {
    setDownloadProgress(prev => ({ ...prev, [workshopId]: 0 }));
    enqueueDownloads([{ workshopId, title, imagePath }], 'single-download');
  };

  // Delete addon(s)
  const deleteAddons = async (ids: string[], deleteFile: boolean, removeFromKnown: boolean) => {
    if (isSubmitting) return;
    
    // Check if any of these ids are local non-workshop items
    const nonWorkshopItems = ids.filter(name => {
      const ad = addons[name] || knownUninstalledAddons[name];
      return ad && !ad.workshopId;
    });

    const executeDelete = async () => {
      setIsSubmitting(true);
      try {
        const data: DatabasePayload = await invoke('delete_addons', { ids, deleteFile, removeFromKnown });
        updateLocalState(data);
        addToast('删除组件成功', 'success');
        handleClearSelection();
      } catch (err) {
        addToast(`删除失败: ${err}`, 'error');
      } finally {
        setIsSubmitting(false);
      }
    };

    if (nonWorkshopItems.length > 0 && deleteFile) {
      // Show warning for non-workshop items
      setConfirmModal({
        open: true,
        title: '删除本地非创意工坊组件',
        message: `您选择的组件中包含 ${nonWorkshopItems.length} 个本地非创意工坊组件。永久删除这些文件后将无法恢复，确定要删除吗？\n\n${nonWorkshopItems.slice(0, 5).join('\n')}${nonWorkshopItems.length > 5 ? '\n...' : ''}`,
        onConfirm: executeDelete
      });
    } else {
      setConfirmModal({
        open: true,
        title: '删除组件',
        message: `确定要删除选中的 ${ids.length} 个组件吗？`,
        onConfirm: executeDelete
      });
    }
  };

  // Selection management
  const handleSelectToggle = (id: string) => {
    setSelectedIds((prev) => {
      const next = prev.includes(id)
        ? prev.filter((name) => name !== id)
        : [...prev, id];
      if (next.length > 0 && !isSelectMode) {
        setIsSelectMode(true);
      }
      return next;
    });
  };

  const handleSelectGroupToggle = (groupAddons: Addon[], groupId?: string) => {
    // In master collection view, toggle the group ID
    if (groupId && currentFilterTab === 'master-collection') {
      setSelectedGroupIds((prev) => {
        const next = prev.includes(groupId)
          ? prev.filter((id) => id !== groupId)
          : [...prev, groupId];
        if (next.length > 0 && !isSelectMode) {
          setIsSelectMode(true);
        }
        return next;
      });
      return;
    }

    // Default: toggle all addons in the group
    const addonIds = groupAddons.map((ad) => ad.id);
    const allSelected = addonIds.every((id) => selectedIds.includes(id));

    if (allSelected) {
      setSelectedIds((prev) => prev.filter((id) => !addonIds.includes(id)));
    } else {
      setSelectedIds((prev) => {
        const next = [...prev];
        addonIds.forEach((id) => {
          if (!next.includes(id)) {
            next.push(id);
          }
        });
        if (next.length > 0 && !isSelectMode) {
          setIsSelectMode(true);
        }
        return next;
      });
    }
  };

  const handleSelectAll = (visibleItems: Addon[]) => {
    const visibleIds = visibleItems.map(item => item.id);
    const allSelected = visibleIds.every(id => selectedIds.includes(id));

    if (allSelected) {
      setSelectedIds((prev) => prev.filter(id => !visibleIds.includes(id)));
    } else {
      setSelectedIds((prev) => {
        const next = [...prev];
        visibleIds.forEach(id => {
          if (!next.includes(id)) {
            next.push(id);
          }
        });
        return next;
      });
    }
  };

  const handleClearSelection = () => {
    setSelectedIds([]);
    setSelectedGroupIds([]);
    setIsSelectMode(false);
  };

  const handleSelectAllGroups = (visibleGroups: Group[]) => {
    const visibleIds = visibleGroups.map(g => g.id);
    const allSelected = visibleIds.every(id => selectedGroupIds.includes(id));

    if (allSelected) {
      setSelectedGroupIds((prev) => prev.filter(id => !visibleIds.includes(id)));
    } else {
      setSelectedGroupIds((prev) => {
        const next = [...prev];
        visibleIds.forEach(id => {
          if (!next.includes(id)) {
            next.push(id);
          }
        });
        return next;
      });
    }
  };

  const handleBatchAddGroupsToMasterCollection = async (mcId: string) => {
    if (selectedGroupIds.length === 0) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-to-master-collection',
        groupId: mcId,
        ids: selectedGroupIds
      });
      updateLocalState(data);
      addToast(t('toasts.addToMasterCollectionSuccess'), 'success');
      handleClearSelection();
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  // Batch actions
  const handleBatchToggle = async (enabled: boolean) => {
    if (selectedIds.length === 0) return;
    if (isSubmitting) return;
    
    const selectedAddonsList = selectedIds.map(name => addons[name] || knownUninstalledAddons[name]).filter(Boolean);
    const actionableAddons = selectedAddonsList.filter(ad => ad.dirType !== 'none' && ad.isEnabled !== enabled);
    if (actionableAddons.length === 0) return;

    setIsSubmitting(true);
    executeWithWorkshopCheck(actionableAddons, enabled ? t('toasts.addonEnabled') : t('toasts.addonDisabled'), async () => {
      try {
        const ids = actionableAddons.map(ad => ad.id);
        const data: DatabasePayload = await invoke('toggle_addons', { 
          ids, 
          enabled 
        });
        updateLocalState(data);
        addToast(enabled ? t('toasts.batchToggleSuccessEnable') : t('toasts.batchToggleSuccessDisable'), 'success');
        handleClearSelection();
      } catch (err) {
        addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
      } finally {
        setIsSubmitting(false);
      }
    });
  };

  const handleBatchMove = async () => {
    if (selectedIds.length === 0) return;
    if (isSubmitting) return;
    
    const movableAddons = selectedIds
      .map(name => addons[name])
      .filter((addon): addon is Addon => Boolean(addon))
      .filter(ad => ad.dirType === 'workshop');
    if (movableAddons.length === 0) return;

    setIsSubmitting(true);
    
    const executeMove = async () => {
      try {
        const ids = movableAddons.map(ad => ad.id);
        const data: DatabasePayload = await invoke('move_addons', { 
          ids, 
          targetDirType: 'loading' 
        });
        updateLocalState(data);
        addToast(t('toasts.batchMoveSuccess'), 'success');
        handleClearSelection();
      } catch (err) {
        addToast(t('toasts.moveFailed', { err: String(err) }), 'error');
      } finally {
        setIsSubmitting(false);
      }
    };

    if (settings.enableDummyBypass) {
      await executeMove();
    } else {
      setConfirmModal({
        open: true,
        title: t('moveWarningModal.title'),
        message: (t('confirmModal.batchMoveMsg', { count: movableAddons.length }) as unknown as string[]).join('\n\n'),
        onConfirm: executeMove
      });
    }
  };

  const handleBatchRename = async () => {
    if (selectedIds.length === 0) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    
    const renamesList: { id: string; newVpkName: string }[] = [];
    const currentAddonsMap = { ...addons };
    
    for (const id of selectedIds) {
      const addon = addons[id];
      if (!addon) continue;
      
      const itemGroup = groups.find(g => g.addons.includes(id));
      const suggestedVpkName = getSuggestedVpkName(addon, itemGroup?.name, currentAddonsMap);
      
      if (suggestedVpkName !== addon.vpkName) {
        renamesList.push({
          id,
          newVpkName: suggestedVpkName
        });
        delete currentAddonsMap[id];
        currentAddonsMap[id] = {
          ...addon,
          vpkName: suggestedVpkName
        };
      }
    }
    
    if (renamesList.length === 0) {
      addToast(t('toasts.batchRenameNoNeed'), 'success');
      setIsSubmitting(false);
      return;
    }

    setConfirmModal({
      open: true,
      title: t('confirmModal.batchRenameTitle'),
      message: t('confirmModal.batchRenameMsg', { count: renamesList.length }),
      onConfirm: async () => {
        const renameTargetAddons = renamesList
          .map(({ id }) => addons[id])
          .filter((addon): addon is Addon => Boolean(addon));
        executeWithWorkshopCheck(renameTargetAddons, t('common.rename'), async () => {
          try {
            const data: DatabasePayload = await invoke('rename_addons', { 
              renames: renamesList 
            });
            updateLocalState(data);
            addToast(t('toasts.batchRenameSuccess', { count: renamesList.length }), 'success');
            handleClearSelection();
          } catch (err) {
            addToast(t('toasts.renameFailed', { err: String(err) }), 'error');
          } finally {
            setIsSubmitting(false);
          }
        });
      }
    });
  };

  const handleBatchAddToGroup = async (groupId: string) => {
    if (selectedIds.length === 0) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        ids: selectedIds
      });
      updateLocalState(data);
      addToast(t('toasts.batchAddGroupSuccess'), 'success');
      handleClearSelection();
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  // Master Collection Management APIs
  const handleCreateMasterCollection = async (name: string, selectedGroupIds?: string[]) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'create-master-collection',
        name,
        ids: selectedGroupIds || []
      });
      updateLocalState(data);
      addToast(t('toasts.createMasterCollectionSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleDeleteMasterCollection = (mcId: string) => {
    setConfirmModal({
      open: true,
      title: t('masterCollections.deleteCollection'),
      message: t('confirmModal.deleteGroupMsg'),
      onConfirm: async () => {
        if (isSubmitting) return;
        setIsSubmitting(true);
        try {
          const data: DatabasePayload = await invoke('group_action', {
            action: 'delete-master-collection',
            groupId: mcId
          });
          updateLocalState(data);
          if (selectedMasterCollectionId === mcId) {
            setSelectedMasterCollectionId(null);
            setCurrentFilterTab('all');
          }
          addToast(t('toasts.deleteMasterCollectionSuccess'), 'success');
        } catch (err) {
          addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
        } finally {
          setIsSubmitting(false);
        }
      }
    });
  };

  const handleRenameMasterCollection = async (mcId: string, newName: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'rename-master-collection',
        groupId: mcId,
        name: newName
      });
      updateLocalState(data);
      addToast(t('toasts.renameMasterCollectionSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleAddGroupToMasterCollection = async (mcId: string, groupIds: string[]) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-to-master-collection',
        groupId: mcId,
        ids: groupIds
      });
      updateLocalState(data);
      addToast(t('toasts.addToMasterCollectionSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleRemoveGroupFromMasterCollection = async (mcId: string, groupIds: string[]) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'remove-from-master-collection',
        groupId: mcId,
        ids: groupIds
      });
      updateLocalState(data);
      addToast(t('toasts.removeFromMasterCollectionSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const ensureSystemMasterCollections = async () => {
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'ensure-system-master-collections'
      });
      updateLocalState(data);
    } catch (err) {
      console.error('Failed to ensure system master collections:', err);
    }
  };

  // Batch download uninstalled addons
  const handleBatchDownload = async (workshopIds: string[]) => {
    if (workshopIds.length === 0) return;
    setDownloadProgress(prev => {
      const next = { ...prev };
      workshopIds.forEach((wId) => {
        next[wId] = next[wId] ?? 0;
      });
      return next;
    });
    enqueueDownloads(workshopIds, 'batch-download');
    addToast(t('toasts.batchDownloadSuccess', { count: workshopIds.length }), 'success');
  };

  // Filter and sort addons
  const getFilteredAddons = () => {
    // For master-collection and groups views, include uninstalled addons so groups with only uninstalled addons are visible
    const includeUninstalled = currentFilterTab === 'master-collection' || currentFilterTab === 'groups';
    let items = currentFilterTab === 'known-uninstalled'
      ? Object.values(knownUninstalledAddons)
      : includeUninstalled
        ? [...Object.values(addons).filter(item => !item.isDummy), ...Object.values(knownUninstalledAddons)]
        : Object.values(addons).filter(item => !item.isDummy);

    // Search query filter
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      items = items.filter(item => {
        const title = (item.steamDetails?.title || getAddonInfoValue(item, 'addontitle') || '').toLowerCase();
        const desc = (item.steamDetails?.description || getAddonInfoValue(item, 'addondescription') || getAddonInfoValue(item, 'addontagline') || '').toLowerCase();
        const author = getAddonAuthor(item).toLowerCase();
        return title.includes(q) || desc.includes(q) || author.includes(q) || item.id.toLowerCase().includes(q) || (item.workshopId || '').includes(q);
      });
    }

    // Category filter
    if (selectedCategory !== 'All') {
      items = items.filter(item => {
        const categories = getAddonCategories(item);
        if (selectedCategory === 'Other') {
          return categories.includes('Other');
        }
        return categories.includes(selectedCategory);
      });
    }

    // Tab filter (if not known-uninstalled)
    if (currentFilterTab !== 'known-uninstalled') {
      if (currentFilterTab === 'workshop') {
        items = items.filter(item => item.dirType === 'workshop');
      } else if (currentFilterTab === 'loading') {
        items = items.filter(item => item.dirType === 'loading');
      } else if (currentFilterTab === 'disabled') {
        items = items.filter(item => !item.isEnabled);
      } else if (currentFilterTab === 'groups' && selectedGroupId) {
        const group = groups.find(g => g.id === selectedGroupId);
        if (group) {
          items = items.filter(item => group.addons.includes(item.id));
        } else {
          items = [];
        }
      } else if (currentFilterTab === 'master-collection' && selectedMasterCollectionId) {
        const mc = masterCollections.find(mc => mc.id === selectedMasterCollectionId);
        if (mc) {
          const groupAddons = new Set<string>();
          for (const gid of mc.groupIds) {
            const group = groups.find(g => g.id === gid);
            if (group) {
              group.addons.forEach(id => groupAddons.add(id));
            }
          }
          items = items.filter(item => groupAddons.has(item.id));
        } else {
          items = [];
        }
      }
    }

    // Sorting
    items.sort((a, b) => {
      if (sortBy === 'title') {
        const titleA = (a.steamDetails?.title || getAddonInfoValue(a, 'addontitle') || a.vpkName).toLowerCase();
        const titleB = (b.steamDetails?.title || getAddonInfoValue(b, 'addontitle') || b.vpkName).toLowerCase();
        return titleA.localeCompare(titleB);
      } else if (sortBy === 'size') {
        return b.fileSize - a.fileSize; // descending size
      } else if (sortBy === 'id') {
        return (b.workshopId || '').localeCompare(a.workshopId || '');
      }
      return 0;
    });

    return items;
  };

  const getRenderedItems = (filteredAddons: Addon[]): RenderedItem[] => {
    if (currentFilterTab === 'groups' || currentFilterTab === 'known-uninstalled') {
      const orderedAddons = currentFilterTab === 'groups'
        ? sortAddonsDownloadedFirst(filteredAddons)
        : filteredAddons;
      return orderedAddons.map(item => ({ type: 'addon' as const, id: item.id, data: item }));
    }

    // For master collection view, show groups within the collection
    if (currentFilterTab === 'master-collection' && selectedMasterCollectionId) {
      const mc = masterCollections.find(mc => mc.id === selectedMasterCollectionId);
      if (mc) {
        const rendered: RenderedItem[] = [];
        const groupedVpkNames = new Set<string>();
        for (const gid of mc.groupIds) {
          const group = groups.find(g => g.id === gid);
          if (group) {
            const matchingAddons = sortAddonsDownloadedFirst(
              filteredAddons.filter(item => group.addons.includes(item.id))
            );
            if (matchingAddons.length > 0) {
              matchingAddons.forEach(item => groupedVpkNames.add(item.id));
              rendered.push({
                type: 'group',
                id: group.id,
                name: group.name,
                addons: matchingAddons,
                groupObj: group
              });
            }
          }
        }
        return rendered;
      }
    }

    const rendered: RenderedItem[] = [];
    const groupedVpkNames = new Set<string>();

    groups.forEach(group => {
      const matchingAddons = sortAddonsDownloadedFirst(
        filteredAddons.filter(item => group.addons.includes(item.id))
      );
      if (matchingAddons.length > 0) {
        matchingAddons.forEach(item => groupedVpkNames.add(item.id));
        rendered.push({
          type: 'group',
          id: group.id,
          name: group.name,
          addons: matchingAddons,
          groupObj: group
        });
      }
    });

    filteredAddons.forEach(item => {
      if (!groupedVpkNames.has(item.id)) {
        rendered.push({
          type: 'addon',
          id: item.id,
          data: item
        });
      }
    });

    rendered.sort((a, b) => {
      if (a.type !== b.type) {
        return a.type === 'group' ? -1 : 1;
      }
      const getSortProps = (item: RenderedItem) => {
        if (item.type === 'addon') {
          const addon = item.data;
          const title = (addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName).toLowerCase();
          const size = addon.fileSize || 0;
          const id = addon.workshopId ?? '';
          return { title, size, id };
        } else {
          const title = item.name.toLowerCase();
          const size = item.addons.reduce((sum, ad) => sum + (ad.fileSize || 0), 0);
          const firstAddonWithId = item.addons.find(ad => ad.workshopId);
          const id = firstAddonWithId?.workshopId ?? '';
          return { title, size, id };
        }
      };

      const propsA = getSortProps(a);
      const propsB = getSortProps(b);

      if (sortBy === 'title') {
        return propsA.title.localeCompare(propsB.title);
      } else if (sortBy === 'size') {
        return propsB.size - propsA.size;
      } else if (sortBy === 'id') {
        return propsB.id.localeCompare(propsA.id);
      }
      return 0;
    });

    return rendered;
  };

  const filteredItems = getFilteredAddons();
  const renderedItems = getRenderedItems(filteredItems);

  // Statistics calculation
  const nonDummyAddons = Object.values(addons).filter(a => !a.isDummy);
  const totalAddonsCount = nonDummyAddons.length;
  const activeCount = nonDummyAddons.filter(a => a.isEnabled).length;
  const disabledCount = totalAddonsCount - activeCount;
  const totalStorageSize = nonDummyAddons.reduce((acc, curr) => acc + (curr.fileSize || 0), 0);

  const currentGroup = currentFilterTab === 'groups' && selectedGroupId
    ? groups.find(g => g.id === selectedGroupId)
    : null;

  const currentMasterCollection = currentFilterTab === 'master-collection' && selectedMasterCollectionId
    ? masterCollections.find(mc => mc.id === selectedMasterCollectionId)
    : null;

  const renameAddonObj = renameModal.currentName ? (addons[renameModal.currentName] || knownUninstalledAddons[renameModal.currentName]) : undefined;
  const renameGroupObj = renameModal.currentName ? groups.find(g => g.addons.includes(renameModal.currentName)) : undefined;

  const handleFilterTabChange = (tab: string, groupId: string | null) => {
    setCurrentFilterTab(tab);
    setSelectedGroupId(groupId);
    if (tab === 'master-collection') {
      setSelectedMasterCollectionId(groupId);
    } else {
      setSelectedMasterCollectionId(null);
    }
    // Clear selections when switching tabs
    setSelectedIds([]);
    setSelectedGroupIds([]);
    setIsSelectMode(false);
  };

  return {
    addons,
    knownUninstalledAddons,
    groups,
    masterCollections,
    settings,
    loading,
    searchQuery,
    setSearchQuery,
    currentFilterTab,
    setCurrentFilterTab,
    selectedGroupId,
    setSelectedGroupId,
    selectedMasterCollectionId,
    setSelectedMasterCollectionId,
    selectedCategory,
    setSelectedCategory,
    sortBy,
    setSortBy,
    toasts,
    setToasts,
    syncingSteam,
    autoGrouping,
    selectedIds,
    selectedGroupIds,
    setSelectedGroupIds,
    isSelectMode,
    setIsSelectMode,
    isSubmitting,
    setIsSubmitting,
    downloadProgress,
    backgroundTasks,

    // Modals state
    detailModal,
    setDetailModal,
    moveWarningModal,
    setMoveWarningModal,
    renameModal,
    setRenameModal,
    groupModal,
    setGroupModal,
    editGroupModal,
    setEditGroupModal,
    settingsModal,
    setSettingsModal,
    linkConfirmModal,
    setLinkConfirmModal,
    confirmModal,
    setConfirmModal,
    workshopActionModal,
    setWorkshopActionModal,

    // Handlers
    fetchData,
    applyDatabaseUpdate: updateLocalState,
    addToast,
    handleOpenLink,
    toggleAddon,
    toggleGroupAddons,
    moveAddon,
    handleMoveClick,
    syncSteamDetails,
    runAutoGrouping,
    saveSettings,
    submitRename,
    handleCreateGroup,
    handleDeleteGroup,
    handleRenameGroup,
    groupActionBatch,
    removeAddonFromGroup,
    addAddonToGroup,
    triggerRenameModal,
    handleSelectToggle,
    handleSelectGroupToggle,
    handleSelectAll,
    handleSelectAllGroups,
    handleClearSelection,
    handleBatchToggle,
    handleBatchMove,
    handleBatchRename,
    handleBatchAddToGroup,
    handleFilterTabChange,
    downloadAddon,
    deleteAddons,
    handleBatchDownload,
    enqueueWorkshopCrawl,
    recordSeenItems,
    cancelTask,
    retryTask,
    clearFinishedTasks,

    // Master Collection handlers
    handleCreateMasterCollection,
    handleDeleteMasterCollection,
    handleRenameMasterCollection,
    handleAddGroupToMasterCollection,
    handleRemoveGroupFromMasterCollection,
    handleBatchAddGroupsToMasterCollection,
    ensureSystemMasterCollections,

    // Derived values
    filteredItems,
    renderedItems,
    totalAddonsCount,
    activeCount,
    disabledCount,
    totalStorageSize,
    currentGroup,
    currentMasterCollection,
    renameAddonObj,
    renameGroupObj,
  };
}

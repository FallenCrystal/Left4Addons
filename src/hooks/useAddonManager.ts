import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Addon, Group, Settings, Toast, DatabasePayload } from '../types/addon';
import { getAddonAuthor, getAddonCategories, getSuggestedVpkName, getAddonInfoValue } from '../utils/addonHelpers';
import { useTranslation } from 'react-i18next';

export type RenderedItem = 
  | { type: 'group'; id: string; name: string; addons: Addon[]; groupObj: Group }
  | { type: 'addon'; id: string; data: Addon };

export function useAddonManager() {
  const { t } = useTranslation();
  const [addons, setAddons] = useState<Record<string, Addon>>({});
  const [knownUninstalledAddons, setKnownUninstalledAddons] = useState<Record<string, Addon>>({});
  const [groups, setGroups] = useState<Group[]>([]);
  const [settings, setSettings] = useState<Settings>({ workshopDir: '', loadingDir: '', enableDummyBypass: false });
  const [loading, setLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState('');
  const [currentFilterTab, setCurrentFilterTab] = useState('all'); // all, workshop, loading, disabled, groups, known-uninstalled, workshop-browser
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState('All');
  const [sortBy, setSortBy] = useState('title'); // title, size, id
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [syncingSteam, setSyncingSteam] = useState(false);
  const [autoGrouping, setAutoGrouping] = useState(false);
  const [selectedVpkNames, setSelectedVpkNames] = useState<string[]>([]);
  const [isSelectMode, setIsSelectMode] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<Record<string, number>>({});

  // Modals state
  const [detailModal, setDetailModal] = useState<{ open: boolean; addon: Addon | null }>({ open: false, addon: null });
  const [moveWarningModal, setMoveWarningModal] = useState<{ open: boolean; vpkName: string; currentDirType: string; workshopId: string }>({ open: false, vpkName: '', currentDirType: '', workshopId: '' });
  const [renameModal, setRenameModal] = useState<{ open: boolean; currentName: string; title: string; suggestedName: string }>({ open: false, currentName: '', title: '', suggestedName: '' });
  const [groupModal, setGroupModal] = useState<{ open: boolean }>({ open: false });
  const [editGroupModal, setEditGroupModal] = useState<{ open: boolean; groupId: string; name: string; tags: string[]; workshopCollectionId: string }>({ open: false, groupId: '', name: '', tags: [], workshopCollectionId: '' });
  const [settingsModal, setSettingsModal] = useState<{ open: boolean; loadingDir: string }>({ open: false, loadingDir: '' });
  const [linkConfirmModal, setLinkConfirmModal] = useState<{ open: boolean; url: string }>({ open: false, url: '' });
  const [confirmModal, setConfirmModal] = useState<{ open: boolean; title: string; message: string; onConfirm: (() => void) | null }>({ open: false, title: '', message: '', onConfirm: null });
  const [workshopActionModal, setWorkshopActionModal] = useState<{ open: boolean; actionName: string; addons: Addon[]; onProceed: (() => void) | null }>({ open: false, actionName: '', addons: [], onProceed: null });

  // Add toast helper
  const addToast = (message: string, type: 'success' | 'error' = 'success') => {
    const id = Math.random().toString(36).substring(2, 11);
    setToasts((prev) => [...prev, { id, message, type }]);
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4000);
  };

  // Listen to download progress
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const setupListener = async () => {
      unlisten = await listen<{ workshopId: string; percent: number }>('download-progress', (event) => {
        setDownloadProgress((prev) => ({
          ...prev,
          [event.payload.workshopId]: event.payload.percent,
        }));
        if (event.payload.percent === 100) {
          setTimeout(() => {
            setDownloadProgress((prev) => {
              const next = { ...prev };
              delete next[event.payload.workshopId];
              return next;
            });
          }, 3000);
        }
      });
    };
    setupListener();
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Update local states from DatabasePayload
  const updateLocalState = (data: DatabasePayload) => {
    setAddons(data.addons || {});
    setGroups(data.groups || []);
    setKnownUninstalledAddons(data.knownUninstalledAddons || {});
    if (data.settings) {
      setSettings(data.settings);
    }
  };

  const executeWithWorkshopCheck = async (addonsList: Addon[], actionName: string, proceed: () => void) => {
    const workshopAddons = addonsList.filter(ad => ad.dirType === 'workshop');
    if (workshopAddons.length > 0) {
      try {
        const vpkNames = workshopAddons.map(a => a.vpkName);
        await invoke('move_addons', { vpkNames, targetDirType: 'loading' });
      } catch (err) {
        addToast(t('toasts.autoMoveFailed', { err: String(err) }), 'error');
        setIsSubmitting(false);
        return;
      }

      if (sessionStorage.getItem('skipWorkshopWarning') !== 'true' && !settings.enableDummyBypass) {
        setWorkshopActionModal({
          open: true,
          actionName,
          addons: workshopAddons,
          onProceed: proceed
        });
      } else {
        proceed();
      }
    } else {
      proceed();
    }
  };

  // Fetch all data
  const fetchData = async (showToastMessage = false) => {
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
  };

  useEffect(() => {
    fetchData();
  }, []);

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
  const toggleAddon = async (vpkName: string, currentStatus: boolean) => {
    if (isSubmitting) return;
    const addon = addons[vpkName] || knownUninstalledAddons[vpkName];
    if (!addon) return;
    
    // If it's not installed, we can't toggle it (needs download)
    if (addon.dirType === 'none') {
      addToast('该组件未安装，请先下载安装', 'error');
      return;
    }

    setIsSubmitting(true);
    executeWithWorkshopCheck([addon], currentStatus ? '禁用组件' : '启用组件', async () => {
      try {
        const data: DatabasePayload = await invoke('toggle_addons', { vpkNames: [vpkName], enabled: !currentStatus });
        updateLocalState(data);
        addToast(currentStatus ? t('toasts.addonDisabled') : t('toasts.addonEnabled'), 'success');
        
        if (detailModal.open && detailModal.addon?.vpkName === vpkName) {
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
    const installedList = addonsList.filter(ad => ad.dirType !== 'none');
    if (installedList.length === 0) return;
    
    setIsSubmitting(true);
    executeWithWorkshopCheck(installedList, enabled ? '批量启用组件' : '批量禁用组件', async () => {
      try {
        const vpkNames = installedList.map(ad => ad.vpkName);
        const data: DatabasePayload = await invoke('toggle_addons', { vpkNames, enabled });
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
  const moveAddon = async (vpkName: string, currentDirType: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    const targetDirType = currentDirType === 'workshop' ? 'loading' : 'workshop';
    try {
      const data: DatabasePayload = await invoke('move_addons', { vpkNames: [vpkName], targetDirType });
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
        moveAddon(addon.vpkName, addon.dirType);
      } else {
        setMoveWarningModal({
          open: true,
          vpkName: addon.vpkName,
          currentDirType: addon.dirType,
          workshopId: addon.workshopId || ''
        });
      }
    } else {
      moveAddon(addon.vpkName, addon.dirType);
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
  const saveSettings = async (loadingDir: string, enableDummyBypass: boolean) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('save_settings', {
        loadingDir,
        enableDummyBypass
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
    const addon = addons[currentName];
    if (!addon) return;
    setIsSubmitting(true);
    executeWithWorkshopCheck([addon], t('common.rename'), async () => {
      try {
        const data: DatabasePayload = await invoke('rename_addon', {
          vpkName: currentName,
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
  const handleCreateGroup = async (name: string, selectedAddons: string[], tags?: string[], workshopCollectionId?: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'create',
        name,
        vpkNames: selectedAddons,
        tags,
        workshopCollectionId
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

  const handleRenameGroup = async (groupId: string, name: string, tags?: string[], workshopCollectionId?: string, vpkNames?: string[]) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'update-group',
        groupId,
        name,
        tags,
        workshopCollectionId,
        vpkNames
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
        const installedList = groupAddons.filter(ad => ad.dirType !== 'none');
        executeWithWorkshopCheck(installedList, enabled ? t('batchActionBar.enable') : t('batchActionBar.disable'), async () => {
          try {
            const vpkNames = installedList.map(ad => ad.vpkName);
            const data: DatabasePayload = await invoke('toggle_addons', { vpkNames, enabled });
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
        const groupAddons = group.addons.map(name => addons[name]).filter(Boolean);
        const isFromWorkshop = groupAddons.some(ad => ad.dirType === 'workshop');
        
        const executeMove = async () => {
          try {
            const vpkNames = groupAddons.map(ad => ad.vpkName);
            const data: DatabasePayload = await invoke('move_addons', { vpkNames, targetDirType });
            updateLocalState(data);
            addToast(t('toasts.batchMoveSuccess'), 'success');
          } catch (err) {
            addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
          } finally {
            setIsSubmitting(false);
          }
        };

        if (isFromWorkshop) {
          if (settings.enableDummyBypass) {
            await executeMove();
          } else {
            setConfirmModal({
              open: true,
              title: t('moveWarningModal.title'),
              message: (t('confirmModal.batchMoveMsg', { count: groupAddons.filter(ad => ad.dirType === 'workshop').length }) as string[]).join('\n\n'),
              onConfirm: executeMove
            });
          }
        } else {
          await executeMove();
        }
      }
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
      setIsSubmitting(false);
    }
  };

  const removeAddonFromGroup = async (vpkName: string, groupId: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'remove-addons',
        groupId,
        vpkNames: [vpkName]
      });
      updateLocalState(data);
      addToast(t('toasts.removeFromGroupSuccess'), 'success');
    } catch (err) {
      addToast(t('toasts.operationFailed', { err: String(err) }), 'error');
    } finally {
      setIsSubmitting(false);
    }
  };

  const addAddonToGroup = async (vpkName: string, groupId: string) => {
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        vpkNames: [vpkName]
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
    const itemGroup = groups.find(g => g.addons.includes(addon.vpkName));
    const suggestedVpkName = getSuggestedVpkName(addon, itemGroup?.name, addons);
    const steamTitle = addon.steamDetails?.title || getAddonInfoValue(addon, 'addontitle') || addon.vpkName;

    setRenameModal({
      open: true,
      currentName: addon.vpkName,
      suggestedName: suggestedVpkName,
      title: steamTitle
    });
  };

  // Download addon
  const downloadAddon = async (workshopId: string) => {
    try {
      setDownloadProgress(prev => ({ ...prev, [workshopId]: 0 }));
      const data: DatabasePayload = await invoke('download_addon', { workshopId });
      updateLocalState(data);
      addToast('下载组件成功', 'success');
    } catch (err) {
      addToast(`下载失败: ${err}`, 'error');
    } finally {
      setDownloadProgress(prev => {
        const next = { ...prev };
        delete next[workshopId];
        return next;
      });
    }
  };

  // Delete addon(s)
  const deleteAddons = async (vpkNames: string[], deleteFile: boolean, removeFromKnown: boolean) => {
    if (isSubmitting) return;
    
    // Check if any of these vpkNames are local non-workshop items
    const nonWorkshopItems = vpkNames.filter(name => {
      const ad = addons[name] || knownUninstalledAddons[name];
      return ad && !ad.workshopId;
    });

    const executeDelete = async () => {
      setIsSubmitting(true);
      try {
        const data: DatabasePayload = await invoke('delete_addons', { vpkNames, deleteFile, removeFromKnown });
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
        message: `确定要删除选中的 ${vpkNames.length} 个组件吗？`,
        onConfirm: executeDelete
      });
    }
  };

  // Selection management
  const handleSelectToggle = (vpkName: string) => {
    setSelectedVpkNames((prev) => {
      const next = prev.includes(vpkName)
        ? prev.filter((name) => name !== vpkName)
        : [...prev, vpkName];
      if (next.length > 0 && !isSelectMode) {
        setIsSelectMode(true);
      }
      return next;
    });
  };

  const handleSelectGroupToggle = (groupAddons: Addon[]) => {
    const addonNames = groupAddons.map((ad) => ad.vpkName);
    const allSelected = addonNames.every((name) => selectedVpkNames.includes(name));

    if (allSelected) {
      setSelectedVpkNames((prev) => prev.filter((name) => !addonNames.includes(name)));
    } else {
      setSelectedVpkNames((prev) => {
        const next = [...prev];
        addonNames.forEach((name) => {
          if (!next.includes(name)) {
            next.push(name);
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
    const visibleNames = visibleItems.map(item => item.vpkName);
    const allSelected = visibleNames.every(name => selectedVpkNames.includes(name));
    
    if (allSelected) {
      setSelectedVpkNames((prev) => prev.filter(name => !visibleNames.includes(name)));
    } else {
      setSelectedVpkNames((prev) => {
        const next = [...prev];
        visibleNames.forEach(name => {
          if (!next.includes(name)) {
            next.push(name);
          }
        });
        return next;
      });
    }
  };

  const handleClearSelection = () => {
    setSelectedVpkNames([]);
    setIsSelectMode(false);
  };

  // Batch actions
  const handleBatchToggle = async (enabled: boolean) => {
    if (selectedVpkNames.length === 0) return;
    if (isSubmitting) return;
    
    const selectedAddonsList = selectedVpkNames.map(name => addons[name] || knownUninstalledAddons[name]).filter(Boolean);
    const installedList = selectedAddonsList.filter(ad => ad.dirType !== 'none');
    if (installedList.length === 0) return;

    setIsSubmitting(true);
    executeWithWorkshopCheck(installedList, enabled ? t('toasts.addonEnabled') : t('toasts.addonDisabled'), async () => {
      try {
        const vpkNames = installedList.map(ad => ad.vpkName);
        const data: DatabasePayload = await invoke('toggle_addons', { 
          vpkNames, 
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
    if (selectedVpkNames.length === 0) return;
    if (isSubmitting) return;
    
    const selectedAddonsList = selectedVpkNames.map(name => addons[name]).filter(Boolean);
    const installedList = selectedAddonsList.filter(ad => ad.dirType !== 'none');
    if (installedList.length === 0) return;

    const workshopAddons = installedList.filter(ad => ad.dirType === 'workshop');
    setIsSubmitting(true);
    
    const executeMove = async () => {
      try {
        const vpkNames = installedList.map(ad => ad.vpkName);
        const data: DatabasePayload = await invoke('move_addons', { 
          vpkNames, 
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

    if (workshopAddons.length > 0) {
      if (settings.enableDummyBypass) {
        await executeMove();
      } else {
        setConfirmModal({
          open: true,
          title: t('moveWarningModal.title'),
          message: (t('confirmModal.batchMoveMsg', { count: workshopAddons.length }) as string[]).join('\n\n'),
          onConfirm: executeMove
        });
      }
    } else {
      await executeMove();
    }
  };

  const handleBatchRename = async () => {
    if (selectedVpkNames.length === 0) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    
    const renamesList: { vpkName: string; newVpkName: string }[] = [];
    const currentAddonsMap = { ...addons };
    
    for (const vpkName of selectedVpkNames) {
      const addon = addons[vpkName];
      if (!addon) continue;
      
      const itemGroup = groups.find(g => g.addons.includes(vpkName));
      const suggestedVpkName = getSuggestedVpkName(addon, itemGroup?.name, currentAddonsMap);
      
      if (suggestedVpkName !== vpkName) {
        renamesList.push({
          vpkName,
          newVpkName: suggestedVpkName
        });
        delete currentAddonsMap[vpkName];
        currentAddonsMap[suggestedVpkName] = {
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
        const selectedAddonsList = selectedVpkNames.map(name => addons[name]).filter(Boolean);
        executeWithWorkshopCheck(selectedAddonsList, t('common.rename'), async () => {
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
    if (selectedVpkNames.length === 0) return;
    if (isSubmitting) return;
    setIsSubmitting(true);
    try {
      const data: DatabasePayload = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        vpkNames: selectedVpkNames
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

  // Filter and sort addons
  const getFilteredAddons = () => {
    let items = currentFilterTab === 'known-uninstalled' 
      ? Object.values(knownUninstalledAddons)
      : Object.values(addons).filter(item => !item.isDummy);

    // Search query filter
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      items = items.filter(item => {
        const title = (item.steamDetails?.title || getAddonInfoValue(item, 'addontitle') || '').toLowerCase();
        const desc = (item.steamDetails?.description || getAddonInfoValue(item, 'addondescription') || getAddonInfoValue(item, 'addontagline') || '').toLowerCase();
        const author = getAddonAuthor(item).toLowerCase();
        return title.includes(q) || desc.includes(q) || author.includes(q) || item.vpkName.toLowerCase().includes(q) || (item.workshopId || '').includes(q);
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
          items = items.filter(item => group.addons.includes(item.vpkName));
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
      return filteredAddons.map(item => ({ type: 'addon' as const, id: item.vpkName, data: item }));
    }

    const rendered: RenderedItem[] = [];
    const groupedVpkNames = new Set<string>();
    
    groups.forEach(group => {
      const matchingAddons = filteredAddons.filter(item => group.addons.includes(item.vpkName));
      if (matchingAddons.length > 0) {
        matchingAddons.forEach(item => groupedVpkNames.add(item.vpkName));
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
      if (!groupedVpkNames.has(item.vpkName)) {
        rendered.push({
          type: 'addon',
          id: item.vpkName,
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

  const renameAddonObj = renameModal.currentName ? (addons[renameModal.currentName] || knownUninstalledAddons[renameModal.currentName]) : undefined;
  const renameGroupObj = renameModal.currentName ? groups.find(g => g.addons.includes(renameModal.currentName)) : undefined;

  const handleFilterTabChange = (tab: string, groupId: string | null) => {
    setCurrentFilterTab(tab);
    setSelectedGroupId(groupId);
  };

  return {
    addons,
    knownUninstalledAddons,
    groups,
    settings,
    loading,
    searchQuery,
    setSearchQuery,
    currentFilterTab,
    setCurrentFilterTab,
    selectedGroupId,
    setSelectedGroupId,
    selectedCategory,
    setSelectedCategory,
    sortBy,
    setSortBy,
    toasts,
    setToasts,
    syncingSteam,
    autoGrouping,
    selectedVpkNames,
    isSelectMode,
    setIsSelectMode,
    isSubmitting,
    setIsSubmitting,
    downloadProgress,
    
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
    handleClearSelection,
    handleBatchToggle,
    handleBatchMove,
    handleBatchRename,
    handleBatchAddToGroup,
    handleFilterTabChange,
    downloadAddon,
    deleteAddons,

    // Derived values
    filteredItems,
    renderedItems,
    totalAddonsCount,
    activeCount,
    disabledCount,
    totalStorageSize,
    currentGroup,
    renameAddonObj,
    renameGroupObj,
  };
}

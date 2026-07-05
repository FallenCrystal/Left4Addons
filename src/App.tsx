import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { 
  FolderOpen, Lock, Unlock, Grid, Edit3, Trash2, HardDrive, AlertCircle, CheckCircle, FileText, RefreshCw,
  FolderPlus, CheckSquare, X
} from 'lucide-react';

// Types & Utilities
import { Addon, Group, Settings, Toast } from './types/addon';
import { formatBytes, getAddonAuthor, getAddonCategories, getSuggestedVpkName } from './utils/addonHelpers';

// Components
import { Sidebar } from './components/Sidebar';
import { TopBar } from './components/TopBar';
import { AddonCard } from './components/AddonCard';
import { GroupCard } from './components/GroupCard';
import { ConfirmModal } from './components/ConfirmModal';
import { MoveWarningModal } from './components/MoveWarningModal';
import { LinkConfirmModal } from './components/LinkConfirmModal';
import { RenameModal } from './components/RenameModal';
import { GroupModal } from './components/GroupModal';
import { EditGroupModal } from './components/EditGroupModal';
import { SettingsModal } from './components/SettingsModal';
import { DetailModal } from './components/DetailModal';
import { WorkshopActionWarningModal } from './components/WorkshopActionWarningModal';

type RenderedItem = 
  | { type: 'group'; id: string; name: string; addons: Addon[]; groupObj: Group }
  | { type: 'addon'; id: string; data: Addon };

function App() {
  const [addons, setAddons] = useState<Record<string, Addon>>({});
  const [groups, setGroups] = useState<Group[]>([]);
  const [settings, setSettings] = useState<Settings>({ workshopDir: '', loadingDir: '' });
  const [loading, setLoading] = useState(true);
  const [searchQuery, setSearchQuery] = useState('');
  const [currentFilterTab, setCurrentFilterTab] = useState('all'); // all, workshop, loading, disabled, groups
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [selectedCategory, setSelectedCategory] = useState('All');
  const [sortBy, setSortBy] = useState('title'); // title, size, id
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [syncingSteam, setSyncingSteam] = useState(false);
  const [autoGrouping, setAutoGrouping] = useState(false);
  const [selectedVpkNames, setSelectedVpkNames] = useState<string[]>([]);
  const [isSelectMode, setIsSelectMode] = useState(false);

  // Modals state
  const [detailModal, setDetailModal] = useState<{ open: boolean; addon: Addon | null }>({ open: false, addon: null });
  const [moveWarningModal, setMoveWarningModal] = useState<{ open: boolean; vpkName: string; currentDirType: string; workshopId: string }>({ open: false, vpkName: '', currentDirType: '', workshopId: '' });
  const [renameModal, setRenameModal] = useState<{ open: boolean; currentName: string; title: string; suggestedName: string }>({ open: false, currentName: '', title: '', suggestedName: '' });
  const [groupModal, setGroupModal] = useState<{ open: boolean }>({ open: false });
  const [editGroupModal, setEditGroupModal] = useState<{ open: boolean; groupId: string; name: string }>({ open: false, groupId: '', name: '' });
  const [settingsModal, setSettingsModal] = useState<{ open: boolean; loadingDir: string }>({ open: false, loadingDir: '' });
  const [linkConfirmModal, setLinkConfirmModal] = useState<{ open: boolean; url: string }>({ open: false, url: '' });
  const [confirmModal, setConfirmModal] = useState<{ open: boolean; title: string; message: string; onConfirm: (() => void) | null }>({ open: false, title: '', message: '', onConfirm: null });
  const [workshopActionModal, setWorkshopActionModal] = useState<{ open: boolean; actionName: string; addons: Addon[]; onProceed: (() => void) | null }>({ open: false, actionName: '', addons: [], onProceed: null });

  const executeWithWorkshopCheck = async (addonsList: Addon[], actionName: string, proceed: () => void) => {
    const workshopAddons = addonsList.filter(ad => ad.dirType === 'workshop');
    if (workshopAddons.length > 0) {
      try {
        const vpkNames = workshopAddons.map(a => a.vpkName);
        await invoke('move_addons', { vpkNames, targetDirType: 'loading' });
      } catch (err) {
        addToast('自动移动附件失败: ' + err, 'error');
        return; // Stop if move fails
      }

      if (sessionStorage.getItem('skipWorkshopWarning') !== 'true') {
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
      const data: { addons?: Record<string, Addon>; groups?: Group[]; settings?: Settings } = await invoke('get_addons');
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      setSettings(data.settings || { workshopDir: '', loadingDir: '' });
      if (showToastMessage) {
        addToast('数据库刷新成功', 'success');
      }
    } catch (err) {
      console.error(err);
      addToast('数据加载失败: ' + err, 'error');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
  }, []);

  // Add toast helper
  const addToast = (message: string, type: 'success' | 'error' = 'success') => {
    const id = Math.random().toString(36).substr(2, 9);
    setToasts((prev) => [...prev, { id, message, type }]);
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4000);
  };

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
    const addon = addons[vpkName];
    if (!addon) return;
    executeWithWorkshopCheck([addon], currentStatus ? '禁用组件' : '启用组件', async () => {
      try {
        const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('toggle_addons', { vpkNames: [vpkName], enabled: !currentStatus });
        setAddons(data.addons || {});
        setGroups(data.groups || []);
        addToast(!currentStatus ? '附加组件已启用' : '附加组件已禁用', 'success');
        
        // Update detail modal state in-place if open
        if (detailModal.open && detailModal.addon?.vpkName === vpkName) {
          setDetailModal(prev => ({
            ...prev,
            addon: prev.addon ? { ...prev.addon, isEnabled: !currentStatus } : null
          }));
        }
      } catch (err) {
        addToast('操作失败: ' + err, 'error');
      }
    });
  };

  const toggleGroupAddons = async (addonsList: Addon[], enabled: boolean) => {
    executeWithWorkshopCheck(addonsList, enabled ? '批量启用组件' : '批量禁用组件', async () => {
      try {
        const vpkNames = addonsList.map(ad => ad.vpkName);
        const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('toggle_addons', { vpkNames, enabled });
        setAddons(data.addons || {});
        setGroups(data.groups || []);
        addToast(enabled ? '分组内所有组件已启用' : '分组内所有组件已禁用', 'success');
      } catch (err) {
        addToast('操作失败: ' + err, 'error');
      }
    });
  };

  // Move addon to load or workshop directory
  const moveAddon = async (vpkName: string, currentDirType: string) => {
    const targetDirType = currentDirType === 'workshop' ? 'loading' : 'workshop';
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('move_addons', { vpkNames: [vpkName], targetDirType });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast(targetDirType === 'loading' ? '已移动到手动安装目录' : '已移动到创意工坊目录', 'success');
    } catch (err) {
      addToast('移动失败: ' + err, 'error');
    }
  };

  const handleMoveClick = (addon: Addon) => {
    if (addon.dirType === 'workshop') {
      setMoveWarningModal({
        open: true,
        vpkName: addon.vpkName,
        currentDirType: addon.dirType,
        workshopId: addon.workshopId || ''
      });
    } else {
      moveAddon(addon.vpkName, addon.dirType);
    }
  };

  // Sync Steam Details
  const syncSteamDetails = async () => {
    setSyncingSteam(true);
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('steam_sync');
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast('Steam 同步成功', 'success');
    } catch (err) {
      addToast('同步失败: ' + err, 'error');
    } finally {
      setSyncingSteam(false);
    }
  };

  // Run Auto Grouping
  const runAutoGrouping = async () => {
    setAutoGrouping(true);
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', { action: 'auto-group' });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast('自动归类完成！已发现并组合了战役/地图包。', 'success');
    } catch (err) {
      addToast('自动归类失败: ' + err, 'error');
    } finally {
      setAutoGrouping(false);
    }
  };

  // Save Settings
  const saveSettings = async (loadingDir: string) => {
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[]; settings?: Settings } = await invoke('save_settings', {
        loadingDir
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      setSettings(data.settings || { workshopDir: '', loadingDir: '' });
      setSettingsModal({ open: false, loadingDir: '' });
      addToast('设置保存并扫描成功', 'success');
    } catch (err) {
      addToast('保存失败: ' + err, 'error');
    }
  };

  // Save Rename Addon
  const submitRename = async (currentName: string, newVpkName: string) => {
    const addon = addons[currentName];
    if (!addon) return;
    executeWithWorkshopCheck([addon], '重命名文件', async () => {
      try {
        const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('rename_addon', {
          vpkName: currentName,
          newVpkName
        });
        setAddons(data.addons || {});
        setGroups(data.groups || []);
        setRenameModal({ open: false, currentName: '', title: '', suggestedName: '' });
        addToast('重命名成功', 'success');
      } catch (err) {
        addToast('重命名失败: ' + err, 'error');
      }
    });
  };

  // Group Management APIs
  const handleCreateGroup = async (name: string, selectedAddons: string[]) => {
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', {
        action: 'create',
        name,
        vpkNames: selectedAddons
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      setGroupModal({ open: false });
      addToast('新建分组成功', 'success');
    } catch (err) {
      addToast('创建分组失败: ' + err, 'error');
    }
  };

  const handleDeleteGroup = (groupId: string) => {
    setConfirmModal({
      open: true,
      title: '确认删除分组',
      message: '确定要删除这个分组吗？这不会删除文件，只会解散群组。',
      onConfirm: async () => {
        try {
          const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', { action: 'delete', groupId });
          setAddons(data.addons || {});
          setGroups(data.groups || []);
          if (selectedGroupId === groupId) {
            setSelectedGroupId(null);
            setCurrentFilterTab('all');
          }
          addToast('分组已删除', 'success');
        } catch (err) {
          addToast('删除失败: ' + err, 'error');
        }
      }
    });
  };

  const handleRenameGroup = async (groupId: string, name: string) => {
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', {
        action: 'rename-group',
        groupId,
        name
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      setEditGroupModal({ open: false, groupId: '', name: '' });
      addToast('分组已重命名', 'success');
    } catch (err) {
      addToast('操作失败: ' + err, 'error');
    }
  };

  const groupActionBatch = async (groupId: string, actionType: 'enable' | 'disable' | 'move-load') => {
    const group = groups.find(g => g.id === groupId);
    if (!group) return;
    
    try {
      if (actionType === 'enable' || actionType === 'disable') {
        const enabled = actionType === 'enable';
        const groupAddons = group.addons.map(name => addons[name]).filter(Boolean);
        executeWithWorkshopCheck(groupAddons, enabled ? '批量启用' : '批量禁用', async () => {
          const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('toggle_addons', { vpkNames: group.addons, enabled });
          setAddons(data.addons || {});
          setGroups(data.groups || []);
          addToast(enabled ? '分组内所有组件已启用' : '分组内所有组件已禁用', 'success');
        });
      } else if (actionType === 'move-load') {
        const targetDirType = 'loading';
        const isFromWorkshop = group.addons.some(name => addons[name]?.dirType === 'workshop');
        
        const executeMove = async () => {
          try {
            const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('move_addons', { vpkNames: group.addons, targetDirType });
            setAddons(data.addons || {});
            setGroups(data.groups || []);
            addToast('分组内所有组件已移动到手动安装目录', 'success');
          } catch (err) {
            addToast('批量操作失败: ' + err, 'error');
          }
        };

        if (isFromWorkshop) {
          setConfirmModal({
            open: true,
            title: '移动创意工坊附件提示',
            message: '提示：此分组内含有创意工坊附件。将附件移动到手动安装目录后，建议前往 Steam 创意工坊取消订阅这些附件，否则 Steam 会在下次启动游戏时重新下载。是否继续移动？',
            onConfirm: executeMove
          });
        } else {
          await executeMove();
        }
      }
    } catch (err) {
      addToast('批量操作失败: ' + err, 'error');
    }
  };

  const removeAddonFromGroup = async (vpkName: string, groupId: string) => {
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', {
        action: 'remove-addons',
        groupId,
        vpkNames: [vpkName]
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast('已移出该分组', 'success');
    } catch (err) {
      addToast('操作失败: ' + err, 'error');
    }
  };

  const addAddonToGroup = async (vpkName: string, groupId: string) => {
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        vpkNames: [vpkName]
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast('已加入分组', 'success');
    } catch (err) {
      addToast('操作失败: ' + (err as Error).message, 'error');
    }
  };

  // Helper to suggest workshop title for rename
  const triggerRenameModal = (addon: Addon) => {
    const itemGroup = groups.find(g => g.addons.includes(addon.vpkName));
    const suggestedVpkName = getSuggestedVpkName(addon, itemGroup?.name, addons);
    const steamTitle = addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName;

    setRenameModal({
      open: true,
      currentName: addon.vpkName,
      suggestedName: suggestedVpkName,
      title: steamTitle
    });
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
    const selectedAddonsList = selectedVpkNames.map(name => addons[name]).filter(Boolean);
    executeWithWorkshopCheck(selectedAddonsList, enabled ? '批量启用组件' : '批量禁用组件', async () => {
      try {
        const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('toggle_addons', { 
          vpkNames: selectedVpkNames, 
          enabled 
        });
        setAddons(data.addons || {});
        setGroups(data.groups || []);
        addToast(enabled ? '批量启用成功' : '批量禁用成功', 'success');
        handleClearSelection();
      } catch (err) {
        addToast('操作失败: ' + err, 'error');
      }
    });
  };

  const handleBatchMove = async () => {
    if (selectedVpkNames.length === 0) return;
    const selectedAddonsList = selectedVpkNames.map(name => addons[name]).filter(Boolean);
    const workshopAddons = selectedAddonsList.filter(ad => ad.dirType === 'workshop');
    
    const executeMove = async () => {
      try {
        const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('move_addons', { 
          vpkNames: selectedVpkNames, 
          targetDirType: 'loading' 
        });
        setAddons(data.addons || {});
        setGroups(data.groups || []);
        addToast('已成功批量移动到手动安装目录', 'success');
        handleClearSelection();
      } catch (err) {
        addToast('移动失败: ' + err, 'error');
      }
    };

    if (workshopAddons.length > 0) {
      setConfirmModal({
        open: true,
        title: '移动创意工坊附件提示',
        message: `确定要将选中的 ${workshopAddons.length} 个创意工坊附件移动到手动安装目录吗？移动后建议前往 Steam 创意工坊取消订阅这些附件，否则 Steam 会在下次启动游戏时重新下载。`,
        onConfirm: executeMove
      });
    } else {
      executeMove();
    }
  };

  const handleBatchRename = async () => {
    if (selectedVpkNames.length === 0) return;
    
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
      addToast('选中的组件已是推荐文件名，无需重命名', 'success');
      return;
    }

    setConfirmModal({
      open: true,
      title: '批量自动重命名',
      message: `确定要自动重命名选中的 ${renamesList.length} 个附件吗？系统将根据创意工坊标题和分组信息自动为它们命名。`,
      onConfirm: async () => {
        const selectedAddonsList = selectedVpkNames.map(name => addons[name]).filter(Boolean);
        executeWithWorkshopCheck(selectedAddonsList, '批量重命名文件', async () => {
          try {
            const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('rename_addons', { 
              renames: renamesList 
            });
            setAddons(data.addons || {});
            setGroups(data.groups || []);
            addToast(`批量重命名成功 (重命名了 ${renamesList.length} 个文件)`, 'success');
            handleClearSelection();
          } catch (err) {
            addToast('重命名失败: ' + err, 'error');
          }
        });
      }
    });
  };

  const handleBatchAddToGroup = async (groupId: string) => {
    if (selectedVpkNames.length === 0) return;
    try {
      const data: { addons?: Record<string, Addon>; groups?: Group[] } = await invoke('group_action', {
        action: 'add-addons',
        groupId,
        vpkNames: selectedVpkNames
      });
      setAddons(data.addons || {});
      setGroups(data.groups || []);
      addToast('批量加入分组成功', 'success');
      handleClearSelection();
    } catch (err) {
      addToast('操作失败: ' + err, 'error');
    }
  };

  // Filter and sort addons
  const getFilteredAddons = () => {
    let items = Object.values(addons);

    // Search query filter
    if (searchQuery) {
      const q = searchQuery.toLowerCase();
      items = items.filter(item => {
        const title = (item.steamDetails?.title || item.addonInfo?.addontitle || '').toLowerCase();
        const desc = (item.steamDetails?.description || item.addonInfo?.addonDescription || item.addonInfo?.addontagline || '').toLowerCase();
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

    // Tab filter
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

    // Sorting
    items.sort((a, b) => {
      if (sortBy === 'title') {
        const titleA = (a.steamDetails?.title || a.addonInfo?.addontitle || a.vpkName).toLowerCase();
        const titleB = (b.steamDetails?.title || b.addonInfo?.addontitle || b.vpkName).toLowerCase();
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
    if (currentFilterTab === 'groups') {
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
          const title = (addon.steamDetails?.title || addon.addonInfo?.addontitle || addon.vpkName).toLowerCase();
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
  const totalAddonsCount = Object.keys(addons).length;
  const activeCount = Object.values(addons).filter(a => a.isEnabled).length;
  const disabledCount = totalAddonsCount - activeCount;
  const totalStorageSize = Object.values(addons).reduce((acc, curr) => acc + (curr.fileSize || 0), 0);

  // Available categories list
  const categoriesList = ['All', 'Campaign', 'Survivor', 'Weapon Model', 'Script', 'Map', 'Skin', 'Sound/Music', 'Infected', 'UI/Textures', 'Other'];

  const handleFilterTabChange = (tab: string, groupId: string | null) => {
    setCurrentFilterTab(tab);
    setSelectedGroupId(groupId);
  };



  const currentGroup = currentFilterTab === 'groups' && selectedGroupId
    ? groups.find(g => g.id === selectedGroupId)
    : null;

  const renameAddonObj = renameModal.currentName ? addons[renameModal.currentName] : undefined;
  const renameGroupObj = renameModal.currentName ? groups.find(g => g.addons.includes(renameModal.currentName)) : undefined;

  return (
    <div className="app-layout">
      {/* Toast Notification Container */}
      <div className="toast-container">
        {toasts.map((toast) => (
          <div key={toast.id} className={`toast ${toast.type === 'error' ? 'toast-error' : 'toast-success'}`}>
            {toast.type === 'error' ? <AlertCircle size={18} /> : <CheckCircle size={18} />}
            <span>{toast.message}</span>
          </div>
        ))}
      </div>

      <Sidebar
        addons={addons}
        groups={groups}
        currentFilterTab={currentFilterTab}
        selectedGroupId={selectedGroupId}
        onFilterTabChange={handleFilterTabChange}
        onOpenGroupModal={() => setGroupModal({ open: true })}
        onOpenSettingsModal={() => setSettingsModal({ open: true, loadingDir: settings.loadingDir })}
        onAutoGrouping={runAutoGrouping}
        autoGrouping={autoGrouping}
        disabledCount={disabledCount}
        totalAddonsCount={totalAddonsCount}
      />

      {/* Main Panel */}
      <div className="main-content">
        <TopBar
          searchQuery={searchQuery}
          onSearchQueryChange={setSearchQuery}
          selectedCategory={selectedCategory}
          onCategoryChange={setSelectedCategory}
          sortBy={sortBy}
          onSortByChange={setSortBy}
          syncingSteam={syncingSteam}
          onSyncSteam={syncSteamDetails}
          categoriesList={categoriesList}
          isSelectMode={isSelectMode}
          onToggleSelectMode={() => {
            if (isSelectMode) {
              handleClearSelection();
            } else {
              setIsSelectMode(true);
            }
          }}
        />

        {/* Content body */}
        <div className="content-body">
          {/* Group details headers */}
          {currentGroup && (
            <div style={{ 
              backgroundColor: 'var(--md-sys-surface-container-high)',
              border: '1px solid var(--md-sys-color-outline-variant)',
              padding: '24px',
              borderRadius: '24px',
              marginBottom: '24px',
              display: 'flex',
              flexDirection: 'column',
              gap: '16px'
            }}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                <div>
                  <h2 style={{ margin: 0, fontSize: '24px', color: '#fff' }}>{currentGroup.name}</h2>
                  <p style={{ margin: '4px 0 0 0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                    此分组包含 {currentGroup.addons?.length || 0} 个附件文件，可进行批量管理。
                  </p>
                </div>
                <div style={{ display: 'flex', gap: '8px' }}>
                  <button 
                    className="btn btn-secondary"
                    onClick={() => setEditGroupModal({ open: true, groupId: currentGroup.id, name: currentGroup.name })}
                  >
                    <Edit3 size={14} />
                    <span>重命名分组</span>
                  </button>
                  <button 
                    className="btn btn-secondary" 
                    style={{ color: 'var(--md-sys-color-error)' }}
                    onClick={() => handleDeleteGroup(currentGroup.id)}
                  >
                    <Trash2 size={14} />
                    <span>解散分组</span>
                  </button>
                </div>
              </div>

              <div style={{ display: 'flex', gap: '8px' }}>
                <button className="btn btn-primary" onClick={() => groupActionBatch(currentGroup.id, 'enable')}>
                  <Unlock size={14} />
                  <span>全部启用</span>
                </button>
                <button className="btn btn-secondary" onClick={() => groupActionBatch(currentGroup.id, 'disable')}>
                  <Lock size={14} />
                  <span>全部禁用</span>
                </button>
                <button className="btn btn-tertiary" onClick={() => groupActionBatch(currentGroup.id, 'move-load')}>
                  <FolderOpen size={14} />
                  <span>全部移动至手动安装</span>
                </button>
              </div>
            </div>
          )}

          {/* Stats bar */}
          {currentFilterTab !== 'groups' && (
            <div className="stats-card-container">
              <div className="stat-card">
                <div className="stat-icon"><Grid size={20} /></div>
                <div className="stat-info">
                  <span className="stat-value">{totalAddonsCount}</span>
                  <span className="stat-label">总附件数</span>
                </div>
              </div>
              <div className="stat-card">
                <div className="stat-icon" style={{ color: 'var(--md-sys-color-success)' }}><CheckCircle size={20} /></div>
                <div className="stat-info">
                  <span className="stat-value">{activeCount}</span>
                  <span className="stat-label">已启用</span>
                </div>
              </div>
              <div className="stat-card">
                <div className="stat-icon" style={{ color: 'var(--md-sys-color-error)' }}><Lock size={20} /></div>
                <div className="stat-info">
                  <span className="stat-value">{disabledCount}</span>
                  <span className="stat-label">已禁用</span>
                </div>
              </div>
              <div className="stat-card">
                <div className="stat-icon"><HardDrive size={20} /></div>
                <div className="stat-info">
                  <span className="stat-value">{formatBytes(totalStorageSize)}</span>
                  <span className="stat-label">总磁盘空间</span>
                </div>
              </div>
            </div>
          )}

          {/* Loading empty states */}
          {loading ? (
            <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '300px', color: 'var(--md-sys-color-outline)' }}>
              <RefreshCw className="animate-spin" size={36} />
              <p style={{ marginTop: '16px' }}>正在解析 VPK 文件和加载数据中...</p>
            </div>
          ) : filteredItems.length === 0 ? (
            <div className="empty-state">
              <FileText className="empty-icon" size={64} />
              <div className="empty-title">未发现匹配的附加组件</div>
              <div className="empty-desc">
                在此筛选条件下未找到任何文件。请确认你的目录中已经放有 L4D2 附件文件。
              </div>
              <button className="btn btn-primary" onClick={() => fetchData(true)}>
                <RefreshCw size={16} />
                <span>重新扫描</span>
              </button>
            </div>
          ) : (
            <div className="addons-grid">
              {renderedItems.map(renderedItem => {
                if (renderedItem.type === 'group') {
                  const groupAddons = renderedItem.addons;
                  const isGroupSelected = groupAddons.every(ad => selectedVpkNames.includes(ad.vpkName));
                  return (
                    <GroupCard
                      key={renderedItem.id}
                      group={renderedItem.groupObj}
                      addons={groupAddons}
                      onToggleGroup={toggleGroupAddons}
                      onViewGroupDetails={(groupId) => {
                        setCurrentFilterTab('groups');
                        setSelectedGroupId(groupId);
                      }}
                      isSelectMode={isSelectMode}
                      isGroupSelected={isGroupSelected}
                      onSelectGroupToggle={handleSelectGroupToggle}
                    />
                  );
                } else {
                  const addonData = renderedItem.data;
                  return (
                    <AddonCard
                      key={renderedItem.id}
                      addon={addonData}
                      groups={groups}
                      onToggle={toggleAddon}
                      onAddToGroup={addAddonToGroup}
                      onRemoveFromGroup={removeAddonFromGroup}
                      onOpenLink={handleOpenLink}
                      onMoveClick={handleMoveClick}
                      onRenameClick={triggerRenameModal}
                      onDetailClick={(addon) => setDetailModal({ open: true, addon })}
                      isSelectMode={isSelectMode}
                      isSelected={selectedVpkNames.includes(addonData.vpkName)}
                      onSelectToggle={handleSelectToggle}
                    />
                  );
                }
              })}
            </div>
          )}
        </div>
      </div>

      {/* Modals */}
      <DetailModal
        open={detailModal.open}
        addon={detailModal.addon ? addons[detailModal.addon.vpkName] : null}
        groups={groups}
        onCancel={() => setDetailModal({ open: false, addon: null })}
        onToggle={toggleAddon}
        onMove={handleMoveClick}
        onOpenLink={handleOpenLink}
      />

      <MoveWarningModal
        open={moveWarningModal.open}
        vpkName={moveWarningModal.vpkName}
        currentDirType={moveWarningModal.currentDirType}
        workshopId={moveWarningModal.workshopId}
        onCancel={() => setMoveWarningModal({ open: false, vpkName: '', currentDirType: '', workshopId: '' })}
        onConfirm={async (vpkName, currentDirType, unsubscribe) => {
          if (unsubscribe && moveWarningModal.workshopId) {
            await invoke('open_url', { url: `steam://url/CommunityFilePage/${moveWarningModal.workshopId}` });
          }
          await moveAddon(vpkName, currentDirType);
          setMoveWarningModal({ open: false, vpkName: '', currentDirType: '', workshopId: '' });
        }}
      />

      <LinkConfirmModal
        open={linkConfirmModal.open}
        url={linkConfirmModal.url}
        onCancel={() => setLinkConfirmModal({ open: false, url: '' })}
        onConfirm={async (url) => {
          try {
            await invoke('open_url', { url });
          } catch (err) {
            console.error('Failed to open link:', err);
          }
          setLinkConfirmModal({ open: false, url: '' });
        }}
      />

      <RenameModal
        open={renameModal.open}
        currentName={renameModal.currentName}
        title={renameModal.title}
        suggestedName={renameModal.suggestedName}
        addon={renameAddonObj}
        itemGroup={renameGroupObj}
        addons={addons}
        onCancel={() => setRenameModal({ open: false, currentName: '', title: '', suggestedName: '' })}
        onConfirm={submitRename}
      />

      <GroupModal
        open={groupModal.open}
        addons={addons}
        onCancel={() => setGroupModal({ open: false })}
        onConfirm={handleCreateGroup}
      />

      <EditGroupModal
        open={editGroupModal.open}
        groupId={editGroupModal.groupId}
        currentName={editGroupModal.name}
        onCancel={() => setEditGroupModal({ open: false, groupId: '', name: '' })}
        onConfirm={handleRenameGroup}
      />

      <SettingsModal
        open={settingsModal.open}
        initialLoadingDir={settingsModal.loadingDir}
        onCancel={() => setSettingsModal({ open: false, loadingDir: '' })}
        onConfirm={saveSettings}
      />

      <ConfirmModal
        open={confirmModal.open}
        title={confirmModal.title}
        message={confirmModal.message}
        onConfirm={confirmModal.onConfirm || (() => {})}
        onCancel={() => setConfirmModal({ open: false, title: '', message: '', onConfirm: null })}
      />

      <WorkshopActionWarningModal
        open={workshopActionModal.open}
        actionName={workshopActionModal.actionName}
        addons={workshopActionModal.addons}
        onCancel={() => setWorkshopActionModal({ open: false, actionName: '', addons: [], onProceed: null })}
        onConfirm={(skipWarning) => {
          if (skipWarning) {
            sessionStorage.setItem('skipWorkshopWarning', 'true');
          }
          if (workshopActionModal.onProceed) {
            workshopActionModal.onProceed();
          }
          setWorkshopActionModal({ open: false, actionName: '', addons: [], onProceed: null });
        }}
      />

      {/* Floating Batch Action Bar */}
      {isSelectMode && (
        <div className="batch-action-bar">
          <div className="batch-action-info">
            已选择 {selectedVpkNames.length} 个组件
          </div>
          <div className="batch-action-buttons">
            <button 
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => handleSelectAll(filteredItems)}
            >
              <CheckSquare size={14} />
              <span>
                {filteredItems.every(item => selectedVpkNames.includes(item.vpkName)) ? '取消全选' : '全选'}
              </span>
            </button>

            <button 
              className="btn btn-primary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => handleBatchToggle(true)}
              disabled={selectedVpkNames.length === 0}
            >
              <Unlock size={14} />
              <span>启用</span>
            </button>

            <button 
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={() => handleBatchToggle(false)}
              disabled={selectedVpkNames.length === 0}
            >
              <Lock size={14} />
              <span>禁用</span>
            </button>

            {selectedVpkNames.some(name => addons[name]?.dirType === 'workshop') && (
              <button 
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                onClick={handleBatchMove}
                disabled={selectedVpkNames.length === 0}
              >
                <FolderOpen size={14} />
                <span>移动至手动安装</span>
              </button>
            )}

            <button 
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
              onClick={handleBatchRename}
              disabled={selectedVpkNames.length === 0}
            >
              <Edit3 size={14} />
              <span>自动重命名</span>
            </button>

            <div className="dropdown">
              <button 
                className="btn btn-secondary"
                style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px' }}
                disabled={selectedVpkNames.length === 0}
              >
                <FolderPlus size={14} />
                <span>加入分组</span>
              </button>
              <div className="dropdown-content">
                {groups.map(g => (
                  <button key={g.id} onClick={() => handleBatchAddToGroup(g.id)}>
                    {g.name}
                  </button>
                ))}
                {groups.length === 0 && (
                  <button disabled style={{ fontStyle: 'italic' }}>无分组</button>
                )}
              </div>
            </div>

            <button 
              className="btn btn-secondary"
              style={{ padding: '6px 12px', fontSize: '12px', display: 'flex', alignItems: 'center', gap: '6px', color: 'var(--md-sys-color-error)' }}
              onClick={handleClearSelection}
            >
              <X size={14} />
              <span>退出批量</span>
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;

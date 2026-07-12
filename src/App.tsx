import { invoke } from '@tauri-apps/api/core';
import { AlertCircle, CheckCircle, RefreshCw, FileText } from 'lucide-react';
import { useAddonManager } from './hooks/useAddonManager';
import { useTranslation } from 'react-i18next';

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
import { SettingsView } from './components/SettingsView';
import { DetailModal } from './components/DetailModal';
import { WorkshopActionWarningModal } from './components/WorkshopActionWarningModal';
import { StatsBar } from './components/StatsBar';
import { GroupHeader } from './components/GroupHeader';
import { BatchActionBar } from './components/BatchActionBar';
import { KnownUninstalledView } from './components/KnownUninstalledView';
import { WorkshopBrowser } from './components/WorkshopBrowser';
import { MasterCollectionModal } from './components/MasterCollectionModal';
import { DeleteConfirmModal } from './components/DeleteConfirmModal';
import { MasterCollectionHeader } from './components/MasterCollectionHeader';
import { AddToGroupModal } from './components/AddToGroupModal';
import { PromptModal } from './components/PromptModal';
import { TaskCenterModal } from './components/TaskCenterModal';
import { useState } from 'react';
import { sortAddonsDownloadedFirst } from './utils/addonHelpers';
import { findKnownWorkshopEntry } from './utils/workshopKnown';

function App() {
  const { t } = useTranslation();
  const {
    addons,
    knownUninstalledAddons,
    groups,
    masterCollections,
    settings,
    loading,
    searchQuery,
    setSearchQuery,
    currentFilterTab,
    selectedGroupId,
    selectedMasterCollectionId,
    selectedCategory,
    setSelectedCategory,
    sortBy,
    setSortBy,
    toasts,
    addToast,
    syncingSteam,
    autoGrouping,
    selectedIds,
    selectedGroupIds,
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
    linkConfirmModal,
    setLinkConfirmModal,
    confirmModal,
    setConfirmModal,
    workshopActionModal,
    setWorkshopActionModal,

    // Handlers
    fetchData,
    applyDatabaseUpdate,
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
    executeRealDelete,
    deleteConfirmModal,
    setDeleteConfirmModal,
    handleBatchDownload,
    recordSeenItems,
    backgroundTasks,
    cancelTask,
    retryTask,
    clearFinishedTasks,

    // Master Collection handlers
    handleCreateMasterCollection,
    handleDeleteMasterCollection,
    handleRenameMasterCollection,
    handleBatchAddGroupsToMasterCollection,

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
  } = useAddonManager();

  const [masterCollectionModal, setMasterCollectionModal] = useState(false);
  const [taskCenterOpen, setTaskCenterOpen] = useState(false);
  const [addToGroupModal, setAddToGroupModal] = useState(false);
  const [singleAddToGroupAddonId, setSingleAddToGroupAddonId] = useState<string | null>(null);
  const [addToMcModal, setAddToMcModal] = useState(false);
  const [renameCollectionModal, setRenameCollectionModal] = useState<{ open: boolean; collectionId: string; currentName: string }>({
    open: false,
    collectionId: '',
    currentName: ''
  });

  // Available categories list
  const categoriesList = [
    'All', 'Campaign', 'Survivor', 'Weapon Model', 'Script',
    'Map', 'Skin', 'Sound/Music', 'Infected', 'UI/Textures', 'Other'
  ];

  // Get all addons (installed + uninstalled) for group rendering
  const getAllGroupAddons = (groupAddonIds: string[]) => {
    return sortAddonsDownloadedFirst(
      groupAddonIds
      .map(id => addons[id] || knownUninstalledAddons[id])
      .filter(Boolean)
    );
  };

  // Get groups in current master collection
  const groupsInMasterCollection = currentMasterCollection
    ? groups.filter(g => currentMasterCollection.groupIds.includes(g.id))
    : [];

  const resolvedDetailModalAddon = detailModal.addon
    ? addons[detailModal.addon.id]
      || knownUninstalledAddons[detailModal.addon.id]
      || findKnownWorkshopEntry(addons, detailModal.addon.workshopId)
      || findKnownWorkshopEntry(knownUninstalledAddons, detailModal.addon.workshopId)
      || detailModal.addon
    : null;

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
        masterCollections={masterCollections}
        currentFilterTab={currentFilterTab}
        selectedGroupId={selectedGroupId}
        selectedMasterCollectionId={selectedMasterCollectionId}
        onFilterTabChange={handleFilterTabChange}
        onOpenGroupModal={() => setGroupModal({ open: true })}
        onOpenMasterCollectionModal={() => setMasterCollectionModal(true)}
        onAutoGrouping={runAutoGrouping}
        autoGrouping={autoGrouping}
        disabledCount={disabledCount}
        totalAddonsCount={totalAddonsCount}
        knownUninstalledCount={Object.keys(knownUninstalledAddons).length}
      />

      {/* Main Panel */}
      <div className="main-content">
        {currentFilterTab === 'settings' ? (
          <SettingsView
            settings={settings}
            isSubmitting={isSubmitting}
            onConfirm={saveSettings}
          />
        ) : currentFilterTab === 'known-uninstalled' ? (
          <KnownUninstalledView
            knownUninstalledAddons={knownUninstalledAddons}
            downloadProgress={downloadProgress}
            onDownload={downloadAddon}
            onDelete={deleteAddons}
            isSubmitting={isSubmitting}
            onOpenLink={handleOpenLink}
            isSelectMode={isSelectMode}
            onToggleSelectMode={() => {
              if (isSelectMode) {
                handleClearSelection();
              } else {
                setIsSelectMode(true);
              }
            }}
            selectedIds={selectedIds}
            onSelectToggle={handleSelectToggle}
            onSelectAll={handleSelectAll}
            onBatchDownload={handleBatchDownload}
            searchQuery={searchQuery}
            onSearchQueryChange={setSearchQuery}
            onDetailClick={(addon) => setDetailModal({ open: true, addon })}
            backgroundTasks={backgroundTasks}
            syncingSteam={syncingSteam}
            onOpenTaskCenter={() => setTaskCenterOpen(true)}
            onWarning={(message: string) => {
              addToast(message, 'error');
            }}
          />
        ) : currentFilterTab === 'workshop-browser' ? (
          <WorkshopBrowser
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            downloadProgress={downloadProgress}
            onDownload={downloadAddon}
            onOpenLink={handleOpenLink}
            onImportCollection={async (name, itemIds) => {
              await handleCreateGroup(name, itemIds, undefined, undefined, 'workshop-import');
            }}
            onRecordSeenItems={recordSeenItems}
            onDatabaseUpdate={applyDatabaseUpdate}
            isSubmitting={isSubmitting}
            groups={groups}
            backgroundTasks={backgroundTasks}
            syncingSteam={syncingSteam}
            onOpenTaskCenter={() => setTaskCenterOpen(true)}
            workshopSourceSettings={settings.workshopSourceSettings}
          />
        ) : (
          <>
            <TopBar
              searchQuery={searchQuery}
              onSearchQueryChange={setSearchQuery}
              selectedCategory={selectedCategory}
              onCategoryChange={setSelectedCategory}
              sortBy={sortBy}
              onSortByChange={setSortBy}
              syncingSteam={syncingSteam}
              categoriesList={categoriesList}
              isSelectMode={isSelectMode}
              onToggleSelectMode={() => {
                if (isSelectMode) {
                  handleClearSelection();
                } else {
                  setIsSelectMode(true);
                }
              }}
              backgroundTasks={backgroundTasks}
              onOpenTaskCenter={() => setTaskCenterOpen(true)}
            />

            {/* Content body */}
            <div className="content-body">
              {/* Master Collection header */}
              {currentMasterCollection && (
                <MasterCollectionHeader
                  currentMasterCollection={currentMasterCollection}
                  groupsInCollection={groupsInMasterCollection}
                  onRenameCollection={() => {
                    setRenameCollectionModal({
                      open: true,
                      collectionId: currentMasterCollection.id,
                      currentName: currentMasterCollection.name
                    });
                  }}
                  onDeleteCollection={() => handleDeleteMasterCollection(currentMasterCollection.id)}
                />
              )}

              {/* Group details header */}
              {currentGroup && (
                <GroupHeader
                  currentGroup={currentGroup}
                  groupAddons={getAllGroupAddons(currentGroup.addons)}
                  onRenameGroup={() => setEditGroupModal({
                    open: true,
                    groupId: currentGroup.id,
                    name: currentGroup.name,
                    tags: currentGroup.tags || [],
                    workshopCollectionId: currentGroup.workshopCollectionId || ''
                  })}
                  onDeleteGroup={() => handleDeleteGroup(currentGroup.id)}
                  onGroupActionBatch={(actionType) => groupActionBatch(currentGroup.id, actionType)}
                  onDownloadUninstalled={handleBatchDownload}
                  downloadProgress={downloadProgress}
                />
              )}

              {/* Stats bar */}
              {currentFilterTab !== 'groups' && currentFilterTab !== 'master-collection' && (
                <StatsBar
                  totalAddonsCount={totalAddonsCount}
                  activeCount={activeCount}
                  disabledCount={disabledCount}
                  totalStorageSize={totalStorageSize}
                />
              )}

              {/* Loading empty states */}
              {loading ? (
                <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '300px', color: 'var(--md-sys-color-outline)' }}>
                  <RefreshCw className="animate-spin" size={36} />
                  <p style={{ marginTop: '16px' }}>{t('common.parsing')}</p>
                </div>
              ) : renderedItems.length === 0 ? (
                <div className="empty-state">
                  <FileText className="empty-icon" size={64} />
                  <div className="empty-title">{t('common.emptyStateTitle')}</div>
                  <div className="empty-desc">
                    {t('common.emptyStateDesc')}
                  </div>
                  <button className="btn btn-primary" onClick={() => fetchData(true)} disabled={loading || isSubmitting}>
                    <RefreshCw size={16} className={loading ? 'animate-spin' : ''} />
                    <span>{loading ? t('common.scanning') : t('common.rescan')}</span>
                  </button>
                </div>
              ) : (
                <div className="addons-grid">
                  {renderedItems.map(renderedItem => {
                    if (renderedItem.type === 'group') {
                      const groupAddons = renderedItem.addons;
                      const allGroupAddons = getAllGroupAddons(renderedItem.groupObj.addons);
                      const isGroupSelected = currentFilterTab === 'master-collection'
                        ? selectedGroupIds.includes(renderedItem.groupObj.id)
                        : groupAddons.every(ad => selectedIds.includes(ad.id));
                      return (
                        <GroupCard
                          key={renderedItem.id}
                          group={renderedItem.groupObj}
                          addons={allGroupAddons}
                          onToggleGroup={toggleGroupAddons}
                          onViewGroupDetails={(groupId) => {
                            handleFilterTabChange('groups', groupId);
                          }}
                          onDownloadUninstalled={handleBatchDownload}
                          isSelectMode={isSelectMode}
                          isGroupSelected={isGroupSelected}
                          onSelectGroupToggle={handleSelectGroupToggle}
                          isSubmitting={isSubmitting}
                          downloadProgress={downloadProgress}
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
                          onAddToGroup={(addonId) => {
                            setSingleAddToGroupAddonId(addonId);
                            setAddToGroupModal(true);
                          }}
                          onOpenLink={handleOpenLink}
                          onMoveClick={handleMoveClick}
                          onRenameClick={triggerRenameModal}
                          onDetailClick={(addon) => setDetailModal({ open: true, addon })}
                          onDeleteClick={(addon) => setDeleteConfirmModal({ open: true, addons: [addon], removeFromKnown: false })}
                          onDownload={downloadAddon}
                          isSelectMode={isSelectMode}
                          isSelected={selectedIds.includes(addonData.id)}
                          onSelectToggle={handleSelectToggle}
                          isSubmitting={isSubmitting}
                          downloadProgress={downloadProgress}
                        />
                      );
                    }
                  })}
                </div>
              )}
            </div>
          </>
        )}
      </div>

      {/* Modals */}
      <DetailModal
        open={detailModal.open}
        addon={resolvedDetailModalAddon}
        groups={groups}
        onCancel={() => setDetailModal({ open: false, addon: null })}
        onToggle={toggleAddon}
        onMove={handleMoveClick}
        onOpenLink={handleOpenLink}
        addons={addons}
        knownUninstalledAddons={knownUninstalledAddons}
        onDatabaseUpdate={applyDatabaseUpdate}
        onDownload={downloadAddon}
        downloadProgress={downloadProgress}
        isSubmitting={isSubmitting}
        onItemNavigate={(workshopId) => {
          const foundAddon = Object.values(addons).find((a) => a.workshopId === workshopId) ||
                             Object.values(knownUninstalledAddons).find((a) => a.workshopId === workshopId);
          if (foundAddon) {
            setDetailModal({ open: true, addon: foundAddon });
          } else {
            setDetailModal({
              open: true,
              addon: {
                id: workshopId,
                vpkName: workshopId,
                workshopId: workshopId,
                dirType: 'none',
                isEnabled: false,
                fileSize: 0,
                filesCount: 0,
              }
            });
          }
        }}
      />

      <MoveWarningModal
        open={moveWarningModal.open}
        id={moveWarningModal.id}
        currentDirType={moveWarningModal.currentDirType}
        workshopId={moveWarningModal.workshopId}
        onCancel={() => {
          setMoveWarningModal({ open: false, id: '', currentDirType: '', workshopId: '' });
          setIsSubmitting(false);
        }}
        onConfirm={async (id, currentDirType, unsubscribe) => {
          if (unsubscribe && moveWarningModal.workshopId) {
            await invoke('open_url', { url: `steam://url/CommunityFilePage/${moveWarningModal.workshopId}` });
          }
          await moveAddon(id, currentDirType);
          setMoveWarningModal({ open: false, id: '', currentDirType: '', workshopId: '' });
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
        isSubmitting={isSubmitting}
        onCancel={() => {
          setRenameModal({ open: false, currentName: '', title: '', suggestedName: '' });
          setIsSubmitting(false);
        }}
        onConfirm={submitRename}
      />

      <GroupModal
        open={groupModal.open}
        isSubmitting={isSubmitting}
        onCancel={() => {
          setGroupModal({ open: false });
          setIsSubmitting(false);
        }}
        onConfirm={(name) => handleCreateGroup(name)}
      />

      <EditGroupModal
        open={editGroupModal.open}
        groupId={editGroupModal.groupId}
        currentName={editGroupModal.name}
        currentTags={editGroupModal.tags}
        currentCollectionId={editGroupModal.workshopCollectionId}
        addonsInGroup={currentGroup ? getAllGroupAddons(currentGroup.addons) : []}
        isSubmitting={isSubmitting}
        onCancel={() => {
          setEditGroupModal({ open: false, groupId: '', name: '', tags: [], workshopCollectionId: '' });
          setIsSubmitting(false);
        }}
        onConfirm={handleRenameGroup}
      />

      <ConfirmModal
        open={confirmModal.open}
        title={confirmModal.title}
        message={confirmModal.message}
        onConfirm={confirmModal.onConfirm || (() => {})}
        onCancel={() => {
          setConfirmModal({ open: false, title: '', message: '', onConfirm: null });
          setIsSubmitting(false);
        }}
      />

      <DeleteConfirmModal
        open={deleteConfirmModal.open}
        addons={deleteConfirmModal.addons}
        isSubmitting={isSubmitting}
        onCancel={() => {
          setDeleteConfirmModal({ open: false, addons: [], removeFromKnown: false });
          setIsSubmitting(false);
        }}
        onConfirm={(deleteMode) => {
          executeRealDelete(deleteMode, deleteConfirmModal.removeFromKnown, deleteConfirmModal.addons.map(a => a.id));
        }}
      />

      <WorkshopActionWarningModal
        open={workshopActionModal.open}
        actionName={workshopActionModal.actionName}
        addons={workshopActionModal.addons}
        onCancel={() => {
          setWorkshopActionModal({ open: false, actionName: '', addons: [], onProceed: null });
          setIsSubmitting(false);
        }}
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

      {/* Master Collection Modal */}
      <MasterCollectionModal
        open={masterCollectionModal}
        isSubmitting={isSubmitting}
        onCancel={() => setMasterCollectionModal(false)}
        onConfirm={async (name) => {
          await handleCreateMasterCollection(name);
          setMasterCollectionModal(false);
        }}
      />

      {/* Rename Master Collection Prompt Modal */}
      <PromptModal
        open={renameCollectionModal.open}
        title={t('masterCollections.renameCollection')}
        defaultValue={renameCollectionModal.currentName}
        placeholder={t('masterCollections.collectionNamePlaceholder')}
        onCancel={() => setRenameCollectionModal({ open: false, collectionId: '', currentName: '' })}
        onConfirm={async (newName) => {
          if (newName.trim()) {
            await handleRenameMasterCollection(renameCollectionModal.collectionId, newName.trim());
          }
          setRenameCollectionModal({ open: false, collectionId: '', currentName: '' });
        }}
      />

      {/* Add to Group Modal */}
      <AddToGroupModal
        open={addToGroupModal}
        groups={groups}
        isSubmitting={isSubmitting}
        onCancel={() => {
          setAddToGroupModal(false);
          setSingleAddToGroupAddonId(null);
        }}
        onConfirm={async (groupId) => {
          if (singleAddToGroupAddonId) {
            await addAddonToGroup(singleAddToGroupAddonId, groupId);
          } else {
            await handleBatchAddToGroup(groupId);
          }
          setAddToGroupModal(false);
          setSingleAddToGroupAddonId(null);
        }}
      />

      {/* Add to Master Collection Modal */}
      <AddToGroupModal
        open={addToMcModal}
        groups={masterCollections.map(mc => ({
          id: mc.id,
          name: mc.nameKey ? t(mc.nameKey, mc.name) : mc.name,
          addons: [],
          masterCollectionIds: [],
        }))}
        isSubmitting={isSubmitting}
        onCancel={() => setAddToMcModal(false)}
        onConfirm={async (mcId) => {
          await handleBatchAddGroupsToMasterCollection(mcId);
          setAddToMcModal(false);
        }}
      />

      {/* Task Center Modal */}
      <TaskCenterModal
        open={taskCenterOpen}
        onCancel={() => setTaskCenterOpen(false)}
        backgroundTasks={backgroundTasks}
        downloadProgress={downloadProgress}
        syncingSteam={syncingSteam}
        onSyncSteam={syncSteamDetails}
        onCancelTask={cancelTask}
        onRetryTask={retryTask}
        onClearFinishedTasks={clearFinishedTasks}
        workshopSourceSettings={settings.workshopSourceSettings}
      />

      {/* Floating Batch Action Bar */}
      {isSelectMode && (
        <BatchActionBar
          selectedIds={selectedIds}
          filteredItems={filteredItems}
          addons={addons}
          knownUninstalledAddons={knownUninstalledAddons}
          groups={groups}
          masterCollections={masterCollections}
          selectedGroupIds={selectedGroupIds}
          onSelectAll={handleSelectAll}
          onSelectAllGroups={handleSelectAllGroups}
          onBatchToggle={handleBatchToggle}
          onBatchMove={handleBatchMove}
          onBatchRename={handleBatchRename}
          onBatchAddToGroup={() => setAddToGroupModal(true)}
          onBatchAddToMasterCollection={() => setAddToMcModal(true)}
          onBatchDownload={handleBatchDownload}
          onBatchDelete={(ids) => {
            const items = ids.map(id => addons[id] || knownUninstalledAddons[id]).filter(Boolean);
            setDeleteConfirmModal({ open: true, addons: items, removeFromKnown: false });
          }}
          onClearSelection={handleClearSelection}
          isSubmitting={isSubmitting}
        />
      )}
    </div>
  );
}

export default App;

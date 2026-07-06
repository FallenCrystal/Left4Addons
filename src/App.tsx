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

function App() {
  const { t } = useTranslation();
  const {
    addons,
    knownUninstalledAddons,
    groups,
    settings,
    loading,
    searchQuery,
    setSearchQuery,
    currentFilterTab,
    selectedGroupId,
    selectedCategory,
    setSelectedCategory,
    sortBy,
    setSortBy,
    toasts,
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
    linkConfirmModal,
    setLinkConfirmModal,
    confirmModal,
    setConfirmModal,
    workshopActionModal,
    setWorkshopActionModal,

    // Handlers
    fetchData,
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
  } = useAddonManager();

  // Available categories list
  const categoriesList = [
    'All', 'Campaign', 'Survivor', 'Weapon Model', 'Script', 
    'Map', 'Skin', 'Sound/Music', 'Infected', 'UI/Textures', 'Other'
  ];

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
          />
        ) : currentFilterTab === 'workshop-browser' ? (
          <WorkshopBrowser
            addons={addons}
            knownUninstalledAddons={knownUninstalledAddons}
            downloadProgress={downloadProgress}
            onDownload={downloadAddon}
            onOpenLink={handleOpenLink}
            onImportCollection={async (name, itemIds) => {
              const vpkNames = itemIds.map(id => `${id}.vpk`);
              await handleCreateGroup(name, vpkNames);
            }}
            isSubmitting={isSubmitting}
            groups={groups}
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
                <GroupHeader
                  currentGroup={currentGroup}
                  onRenameGroup={() => setEditGroupModal({ 
                    open: true, 
                    groupId: currentGroup.id, 
                    name: currentGroup.name,
                    tags: currentGroup.tags || [],
                    workshopCollectionId: currentGroup.workshopCollectionId || ''
                  })}
                  onDeleteGroup={() => handleDeleteGroup(currentGroup.id)}
                  onGroupActionBatch={(actionType) => groupActionBatch(currentGroup.id, actionType)}
                />
              )}

              {/* Stats bar */}
              {currentFilterTab !== 'groups' && (
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
              ) : filteredItems.length === 0 ? (
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
                      const isGroupSelected = groupAddons.every(ad => selectedVpkNames.includes(ad.vpkName));
                      return (
                        <GroupCard
                          key={renderedItem.id}
                          group={renderedItem.groupObj}
                          addons={groupAddons}
                          onToggleGroup={toggleGroupAddons}
                          onViewGroupDetails={(groupId) => {
                            handleFilterTabChange('groups', groupId);
                          }}
                          isSelectMode={isSelectMode}
                          isGroupSelected={isGroupSelected}
                          onSelectGroupToggle={handleSelectGroupToggle}
                          isSubmitting={isSubmitting}
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
                          isSubmitting={isSubmitting}
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
        onCancel={() => {
          setMoveWarningModal({ open: false, vpkName: '', currentDirType: '', workshopId: '' });
          setIsSubmitting(false);
        }}
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
        isSubmitting={isSubmitting}
        onCancel={() => {
          setRenameModal({ open: false, currentName: '', title: '', suggestedName: '' });
          setIsSubmitting(false);
        }}
        onConfirm={submitRename}
      />

      <GroupModal
        open={groupModal.open}
        addons={addons}
        isSubmitting={isSubmitting}
        onCancel={() => {
          setGroupModal({ open: false });
          setIsSubmitting(false);
        }}
        onConfirm={handleCreateGroup}
      />

      <EditGroupModal
        open={editGroupModal.open}
        groupId={editGroupModal.groupId}
        currentName={editGroupModal.name}
        currentTags={editGroupModal.tags}
        currentCollectionId={editGroupModal.workshopCollectionId}
        addonsInGroup={currentGroup ? currentGroup.addons.map(name => addons[name] || knownUninstalledAddons[name]).filter(Boolean) : []}
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

      {/* Floating Batch Action Bar */}
      {isSelectMode && (
        <BatchActionBar
          selectedVpkNames={selectedVpkNames}
          filteredItems={filteredItems}
          addons={addons}
          groups={groups}
          onSelectAll={handleSelectAll}
          onBatchToggle={handleBatchToggle}
          onBatchMove={handleBatchMove}
          onBatchRename={handleBatchRename}
          onBatchAddToGroup={handleBatchAddToGroup}
          onClearSelection={handleClearSelection}
        />
      )}
    </div>
  );
}

export default App;

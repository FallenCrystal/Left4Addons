import React from 'react';
import { Search, CheckSquare, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { CustomSelect } from './CustomSelect';
import { TaskCenterButton } from './TaskCenterButton';
import { BackgroundTask } from '../types/addon';

interface TopBarProps {
  searchQuery: string;
  onSearchQueryChange: (query: string) => void;
  selectedCategory: string;
  onCategoryChange: (category: string) => void;
  sortBy: string;
  onSortByChange: (sortBy: string) => void;
  syncingSteam: boolean;
  categoriesList: string[];
  isSelectMode: boolean;
  onToggleSelectMode: () => void;
  backgroundTasks: BackgroundTask[];
  onOpenTaskCenter: () => void;
}

export const TopBar: React.FC<TopBarProps> = ({
  searchQuery,
  onSearchQueryChange,
  selectedCategory,
  onCategoryChange,
  sortBy,
  onSortByChange,
  syncingSteam,
  categoriesList,
  isSelectMode,
  onToggleSelectMode,
  backgroundTasks,
  onOpenTaskCenter,
}) => {
  const { t } = useTranslation();

  return (
    <div className="top-bar">
      <div className="search-container">
        <Search size={18} className="text-secondary" />
        <input 
          type="text" 
          className="search-input" 
          placeholder={t('topbar.searchPlaceholder')}
          value={searchQuery}
          onChange={(e) => onSearchQueryChange(e.target.value)}
        />
      </div>

      <div className="categories-container">
        {categoriesList.map(cat => (
          <button
            key={cat}
            onClick={() => onCategoryChange(cat)}
            className="btn"
            style={{
              padding: '6px 12px',
              fontSize: '11px',
              borderRadius: '8px',
              backgroundColor: selectedCategory === cat ? 'var(--md-sys-color-primary)' : 'var(--md-sys-surface-container-high)',
              color: selectedCategory === cat ? 'var(--md-sys-color-on-primary)' : 'var(--md-sys-color-on-surface)',
              border: '1px solid var(--md-sys-color-outline-variant)'
            }}
          >
            {t(`categories.${cat}`, cat)}
          </button>
        ))}
      </div>

      <div className="top-bar-actions">
        <CustomSelect
          options={[
            { value: 'title', label: t('topbar.sortByTitle') },
            { value: 'size', label: t('topbar.sortBySize') },
            { value: 'id', label: t('topbar.sortById') },
          ]}
          value={sortBy}
          onChange={onSortByChange}
          minWidth="120px"
        />

        <button
          className={`btn ${isSelectMode ? 'btn-primary' : 'btn-secondary'}`}
          style={{
            height: '42px',
            display: 'flex',
            alignItems: 'center',
            gap: '8px',
            borderRadius: '12px',
            flexShrink: 0,
            padding: '0 16px',
            fontSize: '13px'
          }}
          onClick={onToggleSelectMode}
          title={t('topbar.batchManageTooltip')}
        >
          {isSelectMode ? <X size={16} /> : <CheckSquare size={16} />}
          <span>{isSelectMode ? t('topbar.exitBatch') : t('topbar.batchManage')}</span>
        </button>

        <TaskCenterButton
          syncingSteam={syncingSteam}
          backgroundTasks={backgroundTasks}
          onClick={onOpenTaskCenter}
        />
      </div>
    </div>
  );
};

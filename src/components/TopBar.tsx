import React from 'react';
import { Search, RefreshCw, CheckSquare, X } from 'lucide-react';

interface TopBarProps {
  searchQuery: string;
  onSearchQueryChange: (query: string) => void;
  selectedCategory: string;
  onCategoryChange: (category: string) => void;
  sortBy: string;
  onSortByChange: (sortBy: string) => void;
  syncingSteam: boolean;
  onSyncSteam: () => void;
  categoriesList: string[];
  isSelectMode: boolean;
  onToggleSelectMode: () => void;
}

export const TopBar: React.FC<TopBarProps> = ({
  searchQuery,
  onSearchQueryChange,
  selectedCategory,
  onCategoryChange,
  sortBy,
  onSortByChange,
  syncingSteam,
  onSyncSteam,
  categoriesList,
  isSelectMode,
  onToggleSelectMode,
}) => {
  return (
    <div className="top-bar">
      <div className="search-container">
        <Search size={18} className="text-secondary" />
        <input 
          type="text" 
          className="search-input" 
          placeholder="搜索组件名称, 描述, 作者或创意工坊 ID..." 
          value={searchQuery}
          onChange={(e) => onSearchQueryChange(e.target.value)}
        />
      </div>

      <div className="top-bar-actions">
        <div style={{ display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
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
              {cat}
            </button>
          ))}
        </div>

        <div style={{ width: '1px', height: '24px', backgroundColor: 'var(--md-sys-color-outline-variant)', flexShrink: 0 }}></div>

        <select 
          className="form-input" 
          style={{ padding: '6px 12px', borderRadius: '100px', fontSize: '12px', minWidth: '120px' }}
          value={sortBy}
          onChange={(e) => onSortByChange(e.target.value)}
        >
          <option value="title">按名称排序</option>
          <option value="size">按大小排序</option>
          <option value="id">按创意工坊ID排序</option>
        </select>

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
          title="批量管理附件组件"
        >
          {isSelectMode ? <X size={16} /> : <CheckSquare size={16} />}
          <span>{isSelectMode ? '退出批量' : '批量管理'}</span>
        </button>

        <button 
          className="btn btn-primary btn-icon-only" 
          style={{ width: '42px', height: '42px', display: 'flex', alignItems: 'center', justifyContent: 'center', borderRadius: '12px', flexShrink: 0 }}
          onClick={onSyncSteam}
          disabled={syncingSteam}
          title="刷新所有组件并同步创意工坊信息"
        >
          <RefreshCw size={20} className={syncingSteam ? 'animate-spin' : ''} />
        </button>
      </div>
    </div>
  );
};

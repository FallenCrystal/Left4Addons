/** Workshop item/collection detail drawer */

import React, { useState } from 'react';
import { Download, Globe, FolderPlus, Loader2, FileText } from 'lucide-react';
import { CacheImage } from '../CacheImage';
import { useTranslation } from 'react-i18next';
import { CollectionData } from './types';
import { AlertModal } from '../AlertModal';

interface DetailDrawerProps {
  selectedItem: any | null;
  selectedCollection: CollectionData | null;
  selectedItemLoading: boolean;
  selectedCollectionLoading: boolean;
  downloadProgress: Record<string, number>;
  isSubmitting: boolean;
  onClose: () => void;
  onDownload: (workshopId: string) => void;
  onOpenLink: (url: string) => void;
  onCreatorClick: (id: string, name: string) => void;
  onTagClick: (tagId: string, tagName: string) => void;
  onAddToKnownList: (item: any) => void;
  onImportCollection: (name: string, itemIds: string[]) => void;
}

export const DetailDrawer: React.FC<DetailDrawerProps> = ({
  selectedItem,
  selectedCollection,
  selectedItemLoading,
  selectedCollectionLoading,
  downloadProgress,
  isSubmitting,
  onClose,
  onDownload,
  onOpenLink,
  onCreatorClick,
  onTagClick,
  onAddToKnownList,
  onImportCollection,
}) => {
  const { t } = useTranslation();
  const [alertInfo, setAlertInfo] = useState<{ open: boolean; title: string; message: string }>({
    open: false,
    title: '',
    message: '',
  });

  const handleImportCollectionGroup = () => {
    if (!selectedCollection) return;
    const name = selectedCollection.collection.title || t('workshop.detail.defaultCollectionName');
    const itemIds = selectedCollection.items.map((item) => item.publishedfileid);
    onImportCollection(name, itemIds);
    setAlertInfo({
      open: true,
      title: t('common.success') || '成功',
      message: t('workshop.detail.importSuccess', { count: itemIds.length, name }),
    });
  };

  const handleDownloadAllCollection = () => {
    if (!selectedCollection) return;
    const itemIds = selectedCollection.items.map((item) => item.publishedfileid);
    itemIds.forEach((id) => onDownload(id));
    setAlertInfo({
      open: true,
      title: t('common.success') || '成功',
      message: t('workshop.detail.downloadAllSuccess', { count: itemIds.length }),
    });
  };

  if (!selectedItem && !selectedCollection && !selectedItemLoading && !selectedCollectionLoading) {
    return null;
  }

  return (
    <>
      <div
        style={{
        width: '380px',
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        backgroundColor: 'var(--md-sys-color-surface-container-high)',
        overflowY: 'auto',
        padding: '24px',
        position: 'relative',
        borderLeft: '1px solid var(--md-sys-color-outline-variant)',
      }}
    >
      <button
        onClick={onClose}
        style={{
          position: 'absolute',
          top: '16px',
          right: '16px',
          background: 'none',
          border: 'none',
          fontSize: '20px',
          cursor: 'pointer',
          color: 'var(--md-sys-color-on-surface)',
        }}
      >
        ×
      </button>

      {selectedItemLoading || selectedCollectionLoading ? (
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            height: '100%',
            color: 'var(--md-sys-color-outline)',
          }}
        >
          <Loader2 size={32} className="animate-spin" />
          <p style={{ marginTop: '12px' }}>{t('workshop.detail.loading')}</p>
        </div>
      ) : selectedItem ? (
        <ItemDetail
          item={selectedItem}
          downloadProgress={downloadProgress}
          isSubmitting={isSubmitting}
          onDownload={onDownload}
          onOpenLink={onOpenLink}
          onCreatorClick={onCreatorClick}
          onTagClick={onTagClick}
          onAddToKnownList={onAddToKnownList}
        />
      ) : selectedCollection ? (
        <CollectionDetail
          collection={selectedCollection}
          onOpenLink={onOpenLink}
          onImportGroup={handleImportCollectionGroup}
          onDownloadAll={handleDownloadAllCollection}
        />
      ) : null}
    </div>
  );
};

// ── Item detail sub-component ──────────────────────────────────────────────────

interface ItemDetailProps {
  item: any;
  downloadProgress: Record<string, number>;
  isSubmitting: boolean;
  onDownload: (workshopId: string) => void;
  onOpenLink: (url: string) => void;
  onCreatorClick: (id: string, name: string) => void;
  onTagClick: (tagId: string, tagName: string) => void;
  onAddToKnownList: (item: any) => void;
}

const ItemDetail: React.FC<ItemDetailProps> = ({
  item,
  downloadProgress,
  isSubmitting,
  onDownload,
  onOpenLink,
  onCreatorClick,
  onTagClick,
  onAddToKnownList,
}) => {
  const { t } = useTranslation();

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <CacheImage
        srcPath={item.preview_url}
        alt={item.title}
        style={{
          width: '100%',
          height: '200px',
          objectFit: 'cover',
          borderRadius: '12px',
          marginBottom: '16px',
          backgroundColor: '#111',
        }}
        fallback={
          <div
            style={{
              width: '100%',
              height: '200px',
              borderRadius: '12px',
              marginBottom: '16px',
              backgroundColor: 'var(--md-sys-color-surface-container-high)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: 'var(--md-sys-color-outline)',
            }}
          >
            <FileText size={48} />
          </div>
        }
      />
      <h2
        style={{
          margin: '0 0 8px 0',
          fontSize: '18px',
          fontWeight: 600,
          color: 'var(--md-sys-color-on-surface)',
        }}
      >
        {item.title}
      </h2>
      <div style={{ display: 'flex', alignItems: 'center', gap: '8px', marginBottom: '16px' }}>
        <span style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
          {t('workshop.detail.author')}:
        </span>
        <button
          onClick={() => onCreatorClick(item.creator, item.creator_name || t('workshop.detail.authorPage'))}
          style={{
            background: 'none',
            border: 'none',
            padding: 0,
            fontSize: '13px',
            color: 'var(--md-sys-color-primary)',
            cursor: 'pointer',
            textDecoration: 'underline',
            fontWeight: 500,
          }}
        >
          {item.creator_name || t('workshop.detail.authorPage')}
        </button>
      </div>
      <div style={{ display: 'flex', gap: '6px', flexWrap: 'wrap', marginBottom: '16px' }}>
        {item.tags &&
          item.tags.map((t: any) => (
            <span
              key={t.tag}
              onClick={() => onTagClick(t.tag, t.tag)}
              style={{
                padding: '2px 8px',
                borderRadius: '8px',
                fontSize: '11px',
                fontWeight: 500,
                backgroundColor: 'var(--md-sys-color-secondary-container)',
                color: 'var(--md-sys-color-on-secondary-container)',
                cursor: 'pointer',
              }}
            >
              {t.tag}
            </span>
          ))}
      </div>
      <div
        style={{
          borderTop: '1px solid var(--md-sys-color-outline-variant)',
          paddingTop: '16px',
          marginBottom: '16px',
        }}
      >
        <h4
          style={{
            margin: '0 0 8px 0',
            fontSize: '14px',
            fontWeight: 600,
            color: 'var(--md-sys-color-on-surface)',
          }}
        >
          {t('workshop.detail.description')}
        </h4>
        <p
          style={{
            margin: 0,
            fontSize: '13px',
            color: 'var(--md-sys-color-on-surface-variant)',
            lineHeight: '20px',
            whiteSpace: 'pre-wrap',
            maxHeight: '200px',
            overflowY: 'auto',
          }}
        >
          {item.description}
        </p>
      </div>
      <div style={{ marginTop: 'auto', display: 'flex', flexDirection: 'column', gap: '8px' }}>
        <button
          className="btn btn-primary"
          onClick={() => onDownload(item.publishedfileid)}
          disabled={downloadProgress[item.publishedfileid] !== undefined || isSubmitting}
          style={{
            borderRadius: '100px',
            padding: '12px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: '8px',
            width: '100%',
          }}
        >
          <Download size={16} />
          {downloadProgress[item.publishedfileid] !== undefined
            ? t('workshop.detail.downloading', { progress: downloadProgress[item.publishedfileid] })
            : t('workshop.detail.download')}
        </button>
        <button
          className="btn btn-outline"
          onClick={() => onAddToKnownList(item)}
          style={{
            borderRadius: '100px',
            padding: '12px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: '8px',
            width: '100%',
          }}
        >
          <FolderPlus size={16} /> {t('workshop.detail.addToKnown')}
        </button>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '8px', marginTop: '8px' }}>
          <button
            className="btn btn-outline"
            onClick={() => onOpenLink(`steam://url/CommunityFilePage/${item.publishedfileid}`)}
            style={{
              borderRadius: '100px',
              fontSize: '12px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
            }}
          >
            <Globe size={14} /> {t('workshop.detail.steamProtocol')}
          </button>
          <button
            className="btn btn-outline"
            onClick={() =>
              onOpenLink(
                `https://steamcommunity.com/sharedfiles/filedetails/?id=${item.publishedfileid}`,
              )
            }
            style={{
              borderRadius: '100px',
              fontSize: '12px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
            }}
          >
            <Globe size={14} /> {t('workshop.detail.openInBrowser')}
          </button>
        </div>
      </div>
    </div>
  );
};

// ── Collection detail sub-component ────────────────────────────────────────────

interface CollectionDetailProps {
  collection: CollectionData;
  onOpenLink: (url: string) => void;
  onImportGroup: () => void;
  onDownloadAll: () => void;
}

const CollectionDetail: React.FC<CollectionDetailProps> = ({
  collection,
  onOpenLink,
  onImportGroup,
  onDownloadAll,
}) => {
  const { t } = useTranslation();

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <CacheImage
        srcPath={collection.collection.preview_url}
        alt={collection.collection.title}
        style={{
          width: '100%',
          height: '180px',
          objectFit: 'cover',
          borderRadius: '12px',
          marginBottom: '16px',
          backgroundColor: '#111',
        }}
        fallback={
          <div
            style={{
              width: '100%',
              height: '180px',
              borderRadius: '12px',
              marginBottom: '16px',
              backgroundColor: 'var(--md-sys-color-surface-container-high)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: 'var(--md-sys-color-outline)',
            }}
          >
            <FolderPlus size={48} />
          </div>
        }
      />
      <h2
        style={{
          margin: '0 0 4px 0',
          fontSize: '18px',
          fontWeight: 600,
          color: 'var(--md-sys-color-on-surface)',
        }}
      >
        {collection.collection.title}
      </h2>
      <span style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', marginBottom: '12px' }}>
        {t('workshop.detail.collectionAuthor', {
          author: collection.collection.creator_name || 'Steam',
        })}
      </span>
      <div
        style={{
          borderTop: '1px solid var(--md-sys-color-outline-variant)',
          paddingTop: '12px',
          marginBottom: '12px',
          maxHeight: '100px',
          overflowY: 'auto',
        }}
      >
        <p
          style={{
            margin: 0,
            fontSize: '12px',
            color: 'var(--md-sys-color-on-surface-variant)',
            lineHeight: '18px',
          }}
        >
          {collection.collection.description}
        </p>
      </div>
      <div
        style={{
          flex: 1,
          overflowY: 'auto',
          borderTop: '1px solid var(--md-sys-color-outline-variant)',
          paddingTop: '12px',
          marginBottom: '16px',
        }}
      >
        <h4
          style={{
            margin: '0 0 8px 0',
            fontSize: '13px',
            fontWeight: 600,
            color: 'var(--md-sys-color-on-surface)',
          }}
        >
          {t('workshop.detail.collectionItems', { count: collection.items.length })}
        </h4>
        <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
          {collection.items.map((item) => (
            <div
              key={item.publishedfileid}
              style={{
                display: 'flex',
                gap: '8px',
                alignItems: 'center',
                padding: '6px',
                borderRadius: '8px',
                backgroundColor: 'var(--md-sys-color-surface-container-low)',
              }}
            >
              <CacheImage
                srcPath={item.preview_url}
                alt=""
                style={{
                  width: '40px',
                  height: '40px',
                  objectFit: 'cover',
                  borderRadius: '4px',
                  backgroundColor: '#111',
                }}
                fallback={
                  <div
                    style={{
                      width: '40px',
                      height: '40px',
                      borderRadius: '4px',
                      backgroundColor: 'var(--md-sys-color-surface-container-high)',
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                      color: 'var(--md-sys-color-outline)',
                    }}
                  >
                    <FileText size={20} />
                  </div>
                }
              />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div
                  style={{
                    fontSize: '12px',
                    fontWeight: 500,
                    color: 'var(--md-sys-color-on-surface)',
                    whiteSpace: 'nowrap',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                  }}
                >
                  {item.title}
                </div>
                <div style={{ fontSize: '10px', color: 'var(--md-sys-color-outline)' }}>
                  ID: {item.publishedfileid}
                </div>
              </div>
            </div>
          ))}
        </div>
      </div>
      <div style={{ marginTop: 'auto', display: 'flex', flexDirection: 'column', gap: '8px' }}>
        <button
          className="btn btn-primary"
          onClick={onImportGroup}
          style={{
            borderRadius: '100px',
            padding: '10px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: '8px',
            width: '100%',
            fontSize: '13px',
          }}
        >
          <FolderPlus size={14} /> {t('workshop.detail.importAsGroup')}
        </button>
        <button
          className="btn btn-outline"
          onClick={onDownloadAll}
          style={{
            borderRadius: '100px',
            padding: '10px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: '8px',
            width: '100%',
            fontSize: '13px',
          }}
        >
          <Download size={14} /> {t('workshop.detail.downloadAll')}
        </button>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '6px' }}>
          <button
            className="btn btn-outline"
            onClick={() =>
              onOpenLink(
                `steam://url/CommunityFilePage/${collection.collection.publishedfileid}`,
              )
            }
            style={{
              borderRadius: '100px',
              fontSize: '11px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
            }}
          >
            <Globe size={12} /> {t('workshop.detail.steamProtocol')}
          </button>
          <button
            className="btn btn-outline"
            onClick={() =>
              onOpenLink(
                `https://steamcommunity.com/sharedfiles/filedetails/?id=${collection.collection.publishedfileid}`,
              )
            }
            style={{
              borderRadius: '100px',
              fontSize: '11px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: '4px',
            }}
          >
            <Globe size={12} /> {t('workshop.detail.openInBrowser')}
          </button>
        </div>
      </div>
      <AlertModal
        open={alertInfo.open}
        title={alertInfo.title}
        message={alertInfo.message}
        onClose={() => setAlertInfo({ open: false, title: '', message: '' })}
      />
    </>
  );
};

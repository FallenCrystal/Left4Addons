import React, { useState, useEffect } from 'react';
import { FolderOpen, Info, RefreshCw, FlaskConical, Languages, Check, Cpu, Database, Download } from 'lucide-react';
import { Settings, WorkshopSourceSettings } from '../types/addon';
import { useTranslation } from 'react-i18next';
import { TransHTML } from './TransHTML';

const MIN_DOWNLOAD_CONCURRENCY = 1;
const MAX_DOWNLOAD_CONCURRENCY = 8;

interface SettingsViewProps {
  settings: Settings;
  isSubmitting: boolean;
  onConfirm: (
    loadingDir: string,
    downloadConcurrency: number,
    enableDummyBypass: boolean,
    suppressSdkUnavailableWarning: boolean,
    disableSteamworksSdk: boolean,
    forceSteamworksSdkDownload: boolean,
    maxDownloadRetries: number,
    workshopSourceSettings: WorkshopSourceSettings,
  ) => Promise<void>;
}

const DEFAULT_SOURCE_SETTINGS: WorkshopSourceSettings = {
  preset: 'conservative',
  allowSteamworksSdk: true,
  allowSteamWebApi: true,
  allowSteamCommunityHtml: true,
  allowSdkHtmlHybrid: false,
  sourceOrder: ['steamworks-sdk', 'steam-web-api', 'steamcommunity-html'],
  cacheRetention: 'keep',
};

const SOURCE_LABEL_KEYS: Record<string, string> = {
  'steamworks-sdk': 'settings.sourceSteamworksSdk',
  'steam-web-api': 'settings.sourceSteamWebApi',
  'steamcommunity-html': 'settings.sourceSteamCommunityHtml',
};

function clampDownloadConcurrency(value: number) {
  if (!Number.isFinite(value)) {
    return 2;
  }
  return Math.min(MAX_DOWNLOAD_CONCURRENCY, Math.max(MIN_DOWNLOAD_CONCURRENCY, Math.trunc(value)));
}

export const SettingsView: React.FC<SettingsViewProps> = ({
  settings,
  isSubmitting,
  onConfirm,
}) => {
  const { t, i18n } = useTranslation();
  const [activeTab, setActiveTab] = useState<'path' | 'download' | 'language' | 'experimental' | 'sdk' | 'sources' | 'about'>('path');
  const [loadingDir, setLoadingDir] = useState('');
  const [downloadConcurrencyInput, setDownloadConcurrencyInput] = useState('2');
  const [enableDummyBypass, setEnableDummyBypass] = useState(false);
  const [suppressSdkUnavailableWarning, setSuppressSdkUnavailableWarning] = useState(false);
  const [disableSteamworksSdk, setDisableSteamworksSdk] = useState(false);
  const [forceSteamworksSdkDownload, setForceSteamworksSdkDownload] = useState(false);
  const [maxDownloadRetriesInput, setMaxDownloadRetriesInput] = useState('3');
  const [workshopSourceSettings, setWorkshopSourceSettings] = useState<WorkshopSourceSettings>(DEFAULT_SOURCE_SETTINGS);

  useEffect(() => {
    setLoadingDir(settings.loadingDir || '');
    setDownloadConcurrencyInput(String(clampDownloadConcurrency(settings.downloadConcurrency)));
    setEnableDummyBypass(settings.enableDummyBypass || false);
    setSuppressSdkUnavailableWarning(settings.suppressSdkUnavailableWarning || false);
    setDisableSteamworksSdk(settings.disableSteamworksSdk || false);
    setForceSteamworksSdkDownload(settings.forceSteamworksSdkDownload || false);
    setMaxDownloadRetriesInput(String(settings.maxDownloadRetries ?? 3));
    setWorkshopSourceSettings({
      ...DEFAULT_SOURCE_SETTINGS,
      ...(settings.workshopSourceSettings || {}),
      sourceOrder: settings.workshopSourceSettings?.sourceOrder?.length
        ? settings.workshopSourceSettings.sourceOrder
        : DEFAULT_SOURCE_SETTINGS.sourceOrder,
      cacheRetention: 'keep',
    });
  }, [settings]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!loadingDir.trim() || isSubmitting) return;
    const downloadConcurrency = clampDownloadConcurrency(Number.parseInt(downloadConcurrencyInput, 10));
    setDownloadConcurrencyInput(String(downloadConcurrency));
    
    let maxRetries = Number.parseInt(maxDownloadRetriesInput, 10);
    if (Number.isNaN(maxRetries) || maxRetries < 0) maxRetries = 3;
    if (maxRetries > 20) maxRetries = 20;
    setMaxDownloadRetriesInput(String(maxRetries));

    await onConfirm(
      loadingDir.trim(),
      downloadConcurrency,
      enableDummyBypass,
      suppressSdkUnavailableWarning,
      disableSteamworksSdk,
      forceSteamworksSdkDownload,
      maxRetries,
      workshopSourceSettings,
    );
  };

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    localStorage.setItem('i18n_lang', lng);
  };

  const handleDownloadConcurrencyBlur = () => {
    setDownloadConcurrencyInput(String(clampDownloadConcurrency(Number.parseInt(downloadConcurrencyInput, 10))));
  };

  const handleMaxRetriesBlur = () => {
    let maxRetries = Number.parseInt(maxDownloadRetriesInput, 10);
    if (Number.isNaN(maxRetries) || maxRetries < 0) maxRetries = 3;
    if (maxRetries > 20) maxRetries = 20;
    setMaxDownloadRetriesInput(String(maxRetries));
  };

  const featuresList = t('settings.features', { returnObjects: true }) as string[];

  const updateSourceSettings = (patch: Partial<WorkshopSourceSettings>) => {
    setWorkshopSourceSettings((prev) => ({
      ...prev,
      ...patch,
      cacheRetention: 'keep',
    }));
  };

  const moveSource = (source: string, direction: -1 | 1) => {
    setWorkshopSourceSettings((prev) => {
      const order = [...prev.sourceOrder];
      const index = order.indexOf(source);
      const nextIndex = index + direction;
      if (index < 0 || nextIndex < 0 || nextIndex >= order.length) {
        return prev;
      }
      [order[index], order[nextIndex]] = [order[nextIndex], order[index]];
      return {
        ...prev,
        sourceOrder: order,
        cacheRetention: 'keep',
      };
    });
  };

  const applyPreset = (preset: WorkshopSourceSettings['preset']) => {
    if (preset === 'offline') {
      updateSourceSettings({
        preset,
        allowSteamworksSdk: false,
        allowSteamWebApi: false,
        allowSteamCommunityHtml: false,
        allowSdkHtmlHybrid: false,
      });
      return;
    }
    if (preset === 'sdk-only') {
      updateSourceSettings({
        preset,
        allowSteamworksSdk: true,
        allowSteamWebApi: false,
        allowSteamCommunityHtml: false,
        allowSdkHtmlHybrid: false,
      });
      return;
    }
    if (preset === 'hybrid') {
      updateSourceSettings({
        preset,
        allowSteamworksSdk: true,
        allowSteamWebApi: true,
        allowSteamCommunityHtml: true,
        allowSdkHtmlHybrid: true,
      });
      return;
    }
    updateSourceSettings({
      preset,
      allowSteamworksSdk: true,
      allowSteamWebApi: true,
      allowSteamCommunityHtml: true,
      allowSdkHtmlHybrid: false,
    });
  };

  const downloadCacheDir = '/l4a/cache/downloading';

  return (
    <div className="settings-view">
      {/* Settings Navigation Sidebar */}
      <div className="settings-nav">
        <button
          className={`settings-nav-item ${activeTab === 'path' ? 'active' : ''}`}
          onClick={() => setActiveTab('path')}
          type="button"
        >
          <FolderOpen size={18} />
          <span>{t('settings.pathSettings')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'download' ? 'active' : ''}`}
          onClick={() => setActiveTab('download')}
          type="button"
        >
          <Download size={18} />
          <span>{t('settings.download')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'language' ? 'active' : ''}`}
          onClick={() => setActiveTab('language')}
          type="button"
        >
          <Languages size={18} />
          <span>{t('settings.language')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'experimental' ? 'active' : ''}`}
          onClick={() => setActiveTab('experimental')}
          type="button"
        >
          <FlaskConical size={18} />
          <span>{t('settings.experimental')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'sdk' ? 'active' : ''}`}
          onClick={() => setActiveTab('sdk')}
          type="button"
        >
          <Cpu size={18} />
          <span>{t('settings.sdk')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'sources' ? 'active' : ''}`}
          onClick={() => setActiveTab('sources')}
          type="button"
        >
          <Database size={18} />
          <span>{t('settings.sources')}</span>
        </button>
        <button
          className={`settings-nav-item ${activeTab === 'about' ? 'active' : ''}`}
          onClick={() => setActiveTab('about')}
          type="button"
        >
          <Info size={18} />
          <span>{t('settings.about')}</span>
        </button>
      </div>

      {/* Settings Content Area */}
      <div className="settings-content">
        {activeTab === 'path' && (
          <div>
            <h2 className="settings-title">{t('settings.title')}</h2>
            <form onSubmit={handleSubmit} className="settings-section">
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
                {t('settings.desc')}
              </p>

              <div className="form-group" style={{ marginBottom: '24px' }}>
                <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                  {t('settings.addonsPathLabel')}
                </label>
                <input
                  type="text"
                  className="form-input"
                  value={loadingDir}
                  onChange={(e) => setLoadingDir(e.target.value)}
                  placeholder={t('settings.addonsPathPlaceholder')}
                  style={{ width: '100%' }}
                  required
                  disabled={isSubmitting}
                />
                <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '6px' }}>
                  {t('settings.addonsPathHelp')}
                </span>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>{t('settings.savingAndScanning')}</span>
                    </>
                  ) : (
                    <span>{t('settings.saveAndRescan')}</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}

        {activeTab === 'language' && (
          <div>
            <h2 className="settings-title">{t('settings.languageTitle')}</h2>
            <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '24px', lineHeight: '1.6' }}>
              {t('settings.languageDesc')}
            </p>

            <div className="language-selector-grid">
              <div 
                className={`language-card ${i18n.language === 'zh' ? 'active' : ''}`}
                onClick={() => changeLanguage('zh')}
              >
                <div className="language-card-circle">
                  文
                </div>
                <div className="language-card-info">
                  <div className="language-card-name">简体中文</div>
                  <div className="language-card-sub">Simplified Chinese</div>
                </div>
                {i18n.language === 'zh' && (
                  <div className="language-card-check">
                    <Check size={20} strokeWidth={3} />
                  </div>
                )}
              </div>

              <div 
                className={`language-card ${i18n.language === 'en' ? 'active' : ''}`}
                onClick={() => changeLanguage('en')}
              >
                <div className="language-card-circle">
                  A
                </div>
                <div className="language-card-info">
                  <div className="language-card-name">English</div>
                  <div className="language-card-sub">English</div>
                </div>
                {i18n.language === 'en' && (
                  <div className="language-card-check">
                    <Check size={20} strokeWidth={3} />
                  </div>
                )}
              </div>
            </div>
          </div>
        )}

        {activeTab === 'download' && (
          <div>
            <h2 className="settings-title">{t('settings.downloadTitle')}</h2>
            <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
              {t('settings.downloadDesc')}
            </p>
            <form onSubmit={handleSubmit}>
              <div className="settings-section">
                <div className="form-group" style={{ marginBottom: '24px' }}>
                  <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                    {t('settings.downloadConcurrencyLabel')}
                  </label>
                  <input
                    type="number"
                    className="form-input"
                    value={downloadConcurrencyInput}
                    onChange={(e) => setDownloadConcurrencyInput(e.target.value)}
                    onBlur={handleDownloadConcurrencyBlur}
                    min={MIN_DOWNLOAD_CONCURRENCY}
                    max={MAX_DOWNLOAD_CONCURRENCY}
                    inputMode="numeric"
                    disabled={isSubmitting}
                    style={{ width: '180px' }}
                  />
                  <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '6px' }}>
                    {t('settings.downloadConcurrencyHelp', {
                      min: MIN_DOWNLOAD_CONCURRENCY,
                      max: MAX_DOWNLOAD_CONCURRENCY,
                    })}
                  </span>
                </div>

                <div className="form-group" style={{ marginBottom: '24px' }}>
                  <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                    {t('settings.maxDownloadRetriesLabel')}
                  </label>
                  <input
                    type="number"
                    className="form-input"
                    value={maxDownloadRetriesInput}
                    onChange={(e) => setMaxDownloadRetriesInput(e.target.value)}
                    onBlur={handleMaxRetriesBlur}
                    min={0}
                    max={20}
                    inputMode="numeric"
                    disabled={isSubmitting}
                    style={{ width: '180px' }}
                  />
                  <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '6px' }}>
                    {t('settings.maxDownloadRetriesHelp')}
                  </span>
                </div>

                <div className="form-group">
                  <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                    {t('settings.downloadTempDirLabel')}
                  </label>
                  <input
                    type="text"
                    className="form-input"
                    value={downloadCacheDir}
                    readOnly
                    disabled
                    style={{ width: '100%' }}
                  />
                  <span style={{ fontSize: '11px', color: 'var(--md-sys-color-outline)', display: 'block', marginTop: '6px', lineHeight: '1.5' }}>
                    {t('settings.downloadTempDirHelp')}
                  </span>
                </div>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>{t('settings.savingAndScanning')}</span>
                    </>
                  ) : (
                    <span>{t('settings.saveAndRescan')}</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}

        {activeTab === 'experimental' && (
          <div>
            <h2 className="settings-title">{t('settings.experimentalTitle')}</h2>
            <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
              {t('settings.experimentalDesc')}
            </p>
            <form onSubmit={handleSubmit}>
              <div className="settings-section">
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 0', borderBottom: 'none' }}>
                  <div style={{ paddingRight: '20px' }}>
                    <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                      {t('settings.dummyBypassTitle')}
                    </label>
                    <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', lineHeight: '1.5', display: 'block' }}>
                      <TransHTML i18nKey="settings.dummyBypassDesc" />
                    </div>
                  </div>
                  <label className="switch" style={{ flexShrink: 0 }}>
                    <input
                      type="checkbox"
                      checked={enableDummyBypass}
                      onChange={(e) => setEnableDummyBypass(e.target.checked)}
                      disabled={isSubmitting}
                    />
                    <span className="slider"></span>
                  </label>
                </div>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>{t('settings.savingAndScanning')}</span>
                    </>
                  ) : (
                    <span>{t('settings.saveAndRescan')}</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}

        {activeTab === 'about' && (
          <div>
            <h2 className="settings-title">{t('settings.aboutTitle')}</h2>
            <div className="settings-section" style={{ lineHeight: '1.8' }}>
              <h3 style={{ margin: '0 0 12px 0', fontSize: '18px', color: 'var(--md-sys-color-primary)' }}>
                Left 4 Addons v1.0.0
              </h3>
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-on-surface)', marginBottom: '16px' }}>
                {t('settings.aboutDesc')}
              </p>
              
              <h4 style={{ margin: '20px 0 8px 0', fontSize: '14px', fontWeight: '600' }}>{t('settings.featuresTitle')}</h4>
              <ul style={{ paddingLeft: '20px', margin: '0', fontSize: '13px', color: 'var(--md-sys-color-outline)' }}>
                {featuresList.map((feature: string, idx: number) => (
                  <li key={idx} dangerouslySetInnerHTML={{ __html: feature }} />
                ))}
              </ul>

              <h4 style={{ margin: '20px 0 8px 0', fontSize: '14px', fontWeight: '600' }}>{t('settings.licenseTitle')}</h4>
              <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', margin: '0' }}>
                {t('settings.licenseDesc')}
              </p>
            </div>
          </div>
        )}

        {activeTab === 'sdk' && (
          <div>
            <h2 className="settings-title">{t('settings.sdkTitle')}</h2>
            <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
              {t('settings.sdkDesc')}
            </p>
            <form onSubmit={handleSubmit}>
              <div className="settings-section">
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 0', borderBottom: '1px solid var(--md-sys-color-outline-variant)' }}>
                  <div style={{ paddingRight: '20px' }}>
                    <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                      {t('settings.disableSteamworksSdkTitle')}
                    </label>
                    <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', lineHeight: '1.5', display: 'block' }}>
                      {t('settings.disableSteamworksSdkDesc')}
                    </div>
                  </div>
                  <label className="switch" style={{ flexShrink: 0 }}>
                    <input
                      type="checkbox"
                      checked={disableSteamworksSdk}
                      onChange={(e) => {
                        const checked = e.target.checked;
                        setDisableSteamworksSdk(checked);
                        if (checked) {
                          setForceSteamworksSdkDownload(false);
                        }
                      }}
                      disabled={isSubmitting}
                    />
                    <span className="slider"></span>
                  </label>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 0', borderBottom: 'none' }}>
                  <div style={{ paddingRight: '20px' }}>
                    <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                      {t('settings.forceSteamworksSdkDownloadTitle')}
                    </label>
                    <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', lineHeight: '1.5', display: 'block' }}>
                      {t('settings.forceSteamworksSdkDownloadDesc')}
                    </div>
                  </div>
                  <label className="switch" style={{ flexShrink: 0 }}>
                    <input
                      type="checkbox"
                      checked={forceSteamworksSdkDownload}
                      onChange={(e) => setForceSteamworksSdkDownload(e.target.checked)}
                      disabled={isSubmitting || disableSteamworksSdk}
                    />
                    <span className="slider"></span>
                  </label>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '16px 0', borderBottom: 'none' }}>
                  <div style={{ paddingRight: '20px' }}>
                    <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                      {t('settings.suppressSdkWarningTitle')}
                    </label>
                    <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', lineHeight: '1.5', display: 'block' }}>
                      {t('settings.suppressSdkWarningDesc')}
                    </div>
                  </div>
                  <label className="switch" style={{ flexShrink: 0 }}>
                    <input
                      type="checkbox"
                      checked={suppressSdkUnavailableWarning}
                      onChange={(e) => setSuppressSdkUnavailableWarning(e.target.checked)}
                      disabled={isSubmitting}
                    />
                    <span className="slider"></span>
                  </label>
                </div>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>{t('settings.savingAndScanning')}</span>
                    </>
                  ) : (
                    <span>{t('settings.saveAndRescan')}</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}

        {activeTab === 'sources' && (
          <div>
            <h2 className="settings-title">{t('settings.sourcesTitle')}</h2>
            <p style={{ fontSize: '13px', color: 'var(--md-sys-color-outline)', marginBottom: '20px', lineHeight: '1.6' }}>
              {t('settings.sourcesDesc')}
            </p>
            <form onSubmit={handleSubmit}>
              <div className="settings-section">
                <div className="form-group" style={{ marginBottom: '20px' }}>
                  <label className="form-label" style={{ fontWeight: '600', marginBottom: '8px', display: 'block' }}>
                    {t('settings.sourcePresetLabel')}
                  </label>
                  <select
                    className="form-input"
                    value={workshopSourceSettings.preset}
                    onChange={(event) => applyPreset(event.target.value as WorkshopSourceSettings['preset'])}
                    disabled={isSubmitting}
                    style={{ width: '100%' }}
                  >
                    <option value="conservative">{t('settings.sourcePresetConservative')}</option>
                    <option value="sdk-only">{t('settings.sourcePresetSdkOnly')}</option>
                    <option value="offline">{t('settings.sourcePresetOffline')}</option>
                    <option value="hybrid">{t('settings.sourcePresetHybrid')}</option>
                  </select>
                </div>

                {workshopSourceSettings.allowSdkHtmlHybrid && (
                  <div style={{ padding: '12px 14px', borderRadius: '12px', background: 'var(--md-sys-color-error-container)', color: 'var(--md-sys-color-on-error-container)', fontSize: '12px', lineHeight: '1.5', marginBottom: '16px' }}>
                    {t('settings.sourceHybridWarning')}
                  </div>
                )}

                {[
                  ['allowSteamworksSdk', 'settings.sourceSteamworksSdk'],
                  ['allowSteamWebApi', 'settings.sourceSteamWebApi'],
                  ['allowSteamCommunityHtml', 'settings.sourceSteamCommunityHtml'],
                ].map(([key, labelKey]) => (
                  <div key={key} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '14px 0', borderBottom: '1px solid var(--md-sys-color-outline-variant)' }}>
                    <div style={{ paddingRight: '20px' }}>
                      <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                        {t(labelKey)}
                      </label>
                    </div>
                    <label className="switch" style={{ flexShrink: 0 }}>
                      <input
                        type="checkbox"
                        checked={Boolean(workshopSourceSettings[key as keyof WorkshopSourceSettings])}
                        onChange={(event) => updateSourceSettings({ [key]: event.target.checked } as Partial<WorkshopSourceSettings>)}
                        disabled={isSubmitting || workshopSourceSettings.preset !== 'conservative'}
                      />
                      <span className="slider"></span>
                    </label>
                  </div>
                ))}

                <div style={{ padding: '16px 0', borderBottom: '1px solid var(--md-sys-color-outline-variant)' }}>
                  <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '10px' }}>
                    {t('settings.sourceOrderTitle')}
                  </label>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                    {workshopSourceSettings.sourceOrder.map((source, index) => (
                      <div key={source} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: '12px', padding: '10px 12px', borderRadius: '12px', background: 'var(--md-sys-color-surface-container-high)' }}>
                        <span style={{ fontSize: '13px', fontWeight: 500 }}>{index + 1}. {t(SOURCE_LABEL_KEYS[source] || source)}</span>
                        <div style={{ display: 'flex', gap: '6px' }}>
                          <button
                            type="button"
                            className="btn btn-outline"
                            disabled={isSubmitting || index === 0}
                            onClick={() => moveSource(source, -1)}
                            style={{ height: '30px', padding: '0 10px', borderRadius: '100px', fontSize: '12px' }}
                          >
                            {t('settings.sourceMoveUp')}
                          </button>
                          <button
                            type="button"
                            className="btn btn-outline"
                            disabled={isSubmitting || index === workshopSourceSettings.sourceOrder.length - 1}
                            onClick={() => moveSource(source, 1)}
                            style={{ height: '30px', padding: '0 10px', borderRadius: '100px', fontSize: '12px' }}
                          >
                            {t('settings.sourceMoveDown')}
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '14px 0', borderBottom: 'none' }}>
                  <div style={{ paddingRight: '20px' }}>
                    <label style={{ fontWeight: '600', display: 'block', fontSize: '14px', marginBottom: '4px' }}>
                      {t('settings.sourceAllowHybridTitle')}
                    </label>
                    <div style={{ fontSize: '12px', color: 'var(--md-sys-color-outline)', lineHeight: '1.5', display: 'block' }}>
                      {t('settings.sourceAllowHybridDesc')}
                    </div>
                  </div>
                  <label className="switch" style={{ flexShrink: 0 }}>
                    <input
                      type="checkbox"
                      checked={workshopSourceSettings.allowSdkHtmlHybrid}
                      onChange={(event) => updateSourceSettings({
                        preset: event.target.checked ? 'hybrid' : 'conservative',
                        allowSdkHtmlHybrid: event.target.checked,
                        allowSteamCommunityHtml: event.target.checked || workshopSourceSettings.allowSteamCommunityHtml,
                      })}
                      disabled={isSubmitting}
                    />
                    <span className="slider"></span>
                  </label>
                </div>
              </div>

              <div style={{ display: 'flex', justifyContent: 'flex-start', marginTop: '32px' }}>
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={isSubmitting || !loadingDir.trim()}
                  style={{ minWidth: '160px', height: '42px' }}
                >
                  {isSubmitting ? (
                    <>
                      <RefreshCw className="animate-spin" size={16} />
                      <span>{t('settings.savingAndScanning')}</span>
                    </>
                  ) : (
                    <span>{t('settings.saveAndRescan')}</span>
                  )}
                </button>
              </div>
            </form>
          </div>
        )}
      </div>
    </div>
  );
};

import React, { useState, useEffect } from 'react';
import { FolderOpen, Info, RefreshCw, FlaskConical, Languages, Check, Cpu } from 'lucide-react';
import { Settings } from '../types/addon';
import { useTranslation } from 'react-i18next';
import { TransHTML } from './TransHTML';

interface SettingsViewProps {
  settings: Settings;
  isSubmitting: boolean;
  onConfirm: (
    loadingDir: string,
    enableDummyBypass: boolean,
    suppressSdkUnavailableWarning: boolean,
    disableSteamworksSdk: boolean,
  ) => Promise<void>;
}

export const SettingsView: React.FC<SettingsViewProps> = ({
  settings,
  isSubmitting,
  onConfirm,
}) => {
  const { t, i18n } = useTranslation();
  const [activeTab, setActiveTab] = useState<'path' | 'language' | 'experimental' | 'sdk' | 'about'>('path');
  const [loadingDir, setLoadingDir] = useState('');
  const [enableDummyBypass, setEnableDummyBypass] = useState(false);
  const [suppressSdkUnavailableWarning, setSuppressSdkUnavailableWarning] = useState(false);
  const [disableSteamworksSdk, setDisableSteamworksSdk] = useState(false);

  useEffect(() => {
    setLoadingDir(settings.loadingDir || '');
    setEnableDummyBypass(settings.enableDummyBypass || false);
    setSuppressSdkUnavailableWarning(settings.suppressSdkUnavailableWarning || false);
    setDisableSteamworksSdk(settings.disableSteamworksSdk || false);
  }, [settings]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!loadingDir.trim() || isSubmitting) return;
    await onConfirm(
      loadingDir.trim(),
      enableDummyBypass,
      suppressSdkUnavailableWarning,
      disableSteamworksSdk,
    );
  };

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    localStorage.setItem('i18n_lang', lng);
  };

  const featuresList = t('settings.features', { returnObjects: true }) as string[];

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
                      onChange={(e) => setDisableSteamworksSdk(e.target.checked)}
                      disabled={isSubmitting}
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
      </div>
    </div>
  );
};

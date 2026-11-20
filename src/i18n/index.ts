import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import { en } from './en';
import { zh } from './zh';

const LANG_KEY = 'l4a.v2.lang';

function detectInitialLang(): 'zh' | 'en' {
  if (typeof window !== 'undefined') {
    try {
      const saved = window.localStorage.getItem(LANG_KEY);
      if (saved === 'zh' || saved === 'en') return saved;
    } catch {
      // Ignore storage access errors.
    }
    if (navigator.language?.toLowerCase().startsWith('en')) {
      return 'en';
    }
  }
  return 'zh';
}

export function setAppLanguage(lang: 'zh' | 'en'): void {
  void i18n.changeLanguage(lang);
  if (typeof document !== 'undefined') {
    document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
  }
  if (typeof window !== 'undefined') {
    try {
      window.localStorage.setItem(LANG_KEY, lang);
    } catch {
      // Ignore storage errors.
    }
  }
}

if (!i18n.isInitialized) {
  const lng = detectInitialLang();
  void i18n.use(initReactI18next).init({
    resources: {
      zh: { translation: zh },
      en: { translation: en },
    },
    lng,
    fallbackLng: 'zh',
    interpolation: {
      escapeValue: false,
    },
  });
  if (typeof document !== 'undefined') {
    document.documentElement.lang = lng === 'zh' ? 'zh-CN' : 'en';
  }
}

export default i18n;

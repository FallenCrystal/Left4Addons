import React from 'react';
import { useTranslation } from 'react-i18next';

// i18n Helper component to render multi-line descriptions/warnings securely using array notation (without using \n)
interface TransHTMLProps {
  i18nKey: string;
  values?: Record<string, any>;
  className?: string;
  style?: React.CSSProperties;
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

export const TransHTML: React.FC<TransHTMLProps> = ({ i18nKey, values, className, style }) => {
  const { t } = useTranslation();

  const sanitizedValues = React.useMemo(() => {
    if (!values) return undefined;
    const result: Record<string, any> = {};
    for (const [key, val] of Object.entries(values)) {
      if (typeof val === 'string') {
        result[key] = escapeHtml(val);
      } else {
        result[key] = val;
      }
    }
    return result;
  }, [values]);

  const text = t(i18nKey, sanitizedValues);

  if (Array.isArray(text)) {
    return (
      <div className={className} style={style}>
        {text.map((line: string, idx: number) => (
          <div key={idx} dangerouslySetInnerHTML={{ __html: line }} style={{ marginBottom: idx < text.length - 1 ? '8px' : '0' }} />
        ))}
      </div>
    );
  }

  return <span className={className} style={style} dangerouslySetInnerHTML={{ __html: text as string }} />;
};

interface TransParagraphsProps {
  i18nKey: string;
  values?: Record<string, any>;
}

export const TransParagraphs: React.FC<TransParagraphsProps> = ({ i18nKey, values }) => {
  const { t } = useTranslation();
  const text = t(i18nKey, values);

  if (Array.isArray(text)) {
    return (
      <>
        {text.map((line: string, idx: number) => (
          <React.Fragment key={idx}>
            {line}
            {idx < text.length - 1 && <br />}
          </React.Fragment>
        ))}
      </>
    );
  }

  return <>{text}</>;
};

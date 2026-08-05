import React from 'react';

export const renderTextWithLinks = (
  text: string,
  onDirectNavigate?: (id: string) => void,
  onOpenLink?: (url: string) => void
) => {
  if (!text) return null;
  const urlRegex = /(https?:\/\/[^\s]+)/gi;
  const parts = text.split(urlRegex);
  return parts.map((part, index) => {
    if (part.match(/^https?:\/\//i)) {
      let cleanedUrl = part;
      let trailing = '';
      const matchTrailing = part.match(/([.,;:!?)\]'"]+)$/);
      if (matchTrailing) {
        trailing = matchTrailing[1];
        cleanedUrl = part.slice(0, -trailing.length);
      }
      const steamMatch = cleanedUrl.match(/[?&]id=(\d+)/i);
      const isSteamWorkshop = cleanedUrl.toLowerCase().includes('steamcommunity.com/sharedfiles/filedetails') || 
                            cleanedUrl.toLowerCase().includes('steamcommunity.com/workshop/filedetails');
      if (isSteamWorkshop && steamMatch) {
        const id = steamMatch[1];
        return (
          <React.Fragment key={index}>
            <a
              href="#"
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                if (onDirectNavigate) {
                  onDirectNavigate(id);
                } else {
                  onOpenLink?.(cleanedUrl);
                }
              }}
              style={{ color: 'var(--md-sys-color-primary)', textDecoration: 'underline' }}
            >
              {cleanedUrl}
            </a>
            {trailing}
          </React.Fragment>
        );
      }
      return (
        <React.Fragment key={index}>
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onOpenLink?.(cleanedUrl);
            }}
            style={{ color: 'var(--md-sys-color-primary)', textDecoration: 'underline' }}
          >
            {cleanedUrl}
          </a>
          {trailing}
        </React.Fragment>
      );
    }
    return <span key={index}>{part}</span>;
  });
};

import { WorkshopItem, WorkshopPageDetails } from './types';

interface AuthorDirectoryEntry {
  name?: string;
  url?: string;
  steamId?: string;
  vanityId?: string;
  accountId?: string;
}

type AuthorIdentity = {
  authorName?: string;
  authorUrl?: string;
  authorSteamId?: string;
  authorVanityId?: string;
  authorAccountId?: string;
  authorId?: string;
  ownerSteamId?: string;
  ownerAccountId?: string;
};

const authorDirectory = new Map<string, AuthorDirectoryEntry>();

function normalizeValue(value?: string | null): string {
  return String(value || '').trim();
}

function normalizeKey(type: string, value?: string | null): string | null {
  const trimmed = normalizeValue(value);
  if (!trimmed) return null;
  return `${type}:${trimmed.toLowerCase()}`;
}

function parseProfileIdentifiers(url?: string | null): {
  steamId?: string;
  vanityId?: string;
} {
  const trimmed = normalizeValue(url);
  if (!trimmed) return {};
  return {
    steamId: trimmed.match(/\/profiles\/(\d+)/)?.[1],
    vanityId: trimmed.match(/\/id\/([^/?#]+)/)?.[1],
  };
}

function isNumericIdentifier(value?: string | null): boolean {
  return /^\d+$/.test(normalizeValue(value));
}

function looksLikePlaceholderName(name?: string | null, ids: string[] = []): boolean {
  const trimmed = normalizeValue(name);
  if (!trimmed) return true;
  const lowered = trimmed.toLowerCase();
  if (ids.some((id) => lowered === normalizeValue(id).toLowerCase())) {
    return true;
  }
  return /^\d+$/.test(trimmed);
}

function collectIdentity(input: AuthorIdentity): Required<AuthorIdentity> {
  const parsedFromUrl = parseProfileIdentifiers(input.authorUrl);
  const authorId = normalizeValue(input.authorId);
  const authorSteamId = normalizeValue(input.authorSteamId || input.ownerSteamId || (isNumericIdentifier(authorId) ? authorId : ''));
  const authorVanityId = normalizeValue(
    input.authorVanityId || (!isNumericIdentifier(authorId) ? authorId : '') || parsedFromUrl.vanityId,
  );

  return {
    authorName: normalizeValue(input.authorName),
    authorUrl: normalizeValue(input.authorUrl),
    authorSteamId: authorSteamId || normalizeValue(parsedFromUrl.steamId),
    authorVanityId,
    authorAccountId: normalizeValue(input.authorAccountId || input.ownerAccountId),
    authorId,
    ownerSteamId: normalizeValue(input.ownerSteamId),
    ownerAccountId: normalizeValue(input.ownerAccountId),
  };
}

function buildKeys(identity: Required<AuthorIdentity>): string[] {
  const keys = [
    normalizeKey('steam', identity.authorSteamId),
    normalizeKey('account', identity.authorAccountId),
    normalizeKey('vanity', identity.authorVanityId),
    normalizeKey('id', identity.authorId),
    normalizeKey('url', identity.authorUrl),
  ].filter((key): key is string => Boolean(key));

  return [...new Set(keys)];
}

function lookupAuthor(identity: AuthorIdentity): AuthorDirectoryEntry | null {
  const normalized = collectIdentity(identity);
  for (const key of buildKeys(normalized)) {
    const entry = authorDirectory.get(key);
    if (entry) return entry;
  }
  return null;
}

function rememberAuthor(identity: AuthorIdentity): void {
  const normalized = collectIdentity(identity);
  const keys = buildKeys(normalized);
  if (keys.length === 0) return;

  const existing = keys
    .map((key) => authorDirectory.get(key))
    .find(Boolean);

  const candidateIds = [
    normalized.authorSteamId,
    normalized.authorAccountId,
    normalized.authorVanityId,
    normalized.authorId,
  ].filter(Boolean);

  const merged: AuthorDirectoryEntry = {
    name: looksLikePlaceholderName(normalized.authorName, candidateIds)
      ? existing?.name
      : normalized.authorName || existing?.name,
    url: normalized.authorUrl || existing?.url,
    steamId: normalized.authorSteamId || existing?.steamId,
    vanityId: normalized.authorVanityId || existing?.vanityId,
    accountId: normalized.authorAccountId || existing?.accountId,
  };

  for (const key of keys) {
    authorDirectory.set(key, merged);
  }
}

export function rememberWorkshopItems(items: WorkshopItem[]): WorkshopItem[] {
  items.forEach((item) => rememberAuthor(item));
  return items.map((item) => resolveWorkshopItemAuthor(item));
}

export function rememberWorkshopPageDetails(details: WorkshopPageDetails): void {
  rememberAuthor({
    authorName: details.creatorName,
    authorUrl: details.creatorProfileUrl,
    authorSteamId: details.creatorSteamId,
    authorVanityId: details.creatorVanityId,
    authorAccountId: details.creatorAccountId,
    authorId: details.creatorSteamId || details.creatorVanityId || details.creatorAccountId,
  });
}

export function resolveWorkshopItemAuthor(item: WorkshopItem): WorkshopItem {
  const normalized = collectIdentity(item);
  const known = lookupAuthor(item);
  if (!known) {
    return item;
  }

  const candidateIds = [
    normalized.authorSteamId,
    normalized.authorAccountId,
    normalized.authorVanityId,
    normalized.authorId,
  ].filter(Boolean);

  const authorName = looksLikePlaceholderName(item.authorName, candidateIds)
    ? known.name || item.authorName
    : item.authorName;
  const authorSteamId = item.authorSteamId || known.steamId;
  const authorVanityId = item.authorVanityId || known.vanityId;
  const authorAccountId = item.authorAccountId || known.accountId;
  const authorUrl = item.authorUrl
    || known.url
    || (authorVanityId ? `https://steamcommunity.com/id/${authorVanityId}` : '')
    || (authorSteamId ? `https://steamcommunity.com/profiles/${authorSteamId}` : '');
  const authorId = item.authorId || authorVanityId || authorSteamId || authorAccountId || '';

  if (
    authorName === item.authorName
    && authorUrl === item.authorUrl
    && authorSteamId === item.authorSteamId
    && authorVanityId === item.authorVanityId
    && authorAccountId === item.authorAccountId
    && authorId === item.authorId
  ) {
    return item;
  }

  return {
    ...item,
    authorName,
    authorUrl,
    authorSteamId,
    authorVanityId,
    authorAccountId,
    authorId,
    ownerSteamId: item.ownerSteamId || authorSteamId,
    ownerAccountId: item.ownerAccountId || authorAccountId,
  };
}

export function __resetWorkshopAuthorDirectoryForTests(): void {
  authorDirectory.clear();
}

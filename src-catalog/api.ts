export type WorkshopItemStatus = 'active' | 'removed' | 'private_or_unavailable' | 'unknown';
export type WorkshopFileType = 'item' | 'collection';
export type MetadataSource =
  | 'catalog_server'
  | 'steam_kit'
  | 'steamworks_sdk'
  | 'steam_web_api'
  | 'web_scraper'
  | 'local_vpk'
  | 'admin_override';

export type SteamAuthMode = 'anonymous' | 'account';
export type SteamConnectionState =
  | 'disconnected'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'error';
export type SteamAuthFlowKind = 'qr' | 'credentials';
export type SteamAuthFlowState =
  | 'waiting_for_scan'
  | 'waiting_for_code'
  | 'authenticating'
  | 'succeeded'
  | 'failed';

export interface WorkshopDependency {
  workshopId: string;
  title?: string;
  required: boolean;
}

export interface WorkshopPreview {
  previewId?: string;
  url?: string;
  youtubeVideoId?: string;
  previewType: number;
}

export interface WorkshopItemRecord {
  workshopId: string;
  fileType: WorkshopFileType;
  status: WorkshopItemStatus;
  title: string;
  description?: string;
  creatorSteamId?: string;
  creatorName?: string;
  previewUrl?: string;
  previews?: WorkshopPreview[];
  fileUrl?: string;
  fileSize?: number;
  timeCreated?: number;
  timeUpdated?: number;
  tags: string[];
  dependencies: WorkshopDependency[];
  collectionChildren: string[];
  subscriptions?: number;
  favorites?: number;
  source: MetadataSource;
  fetchedAt: string;
}

export interface SearchResponse {
  items: WorkshopItemRecord[];
  total: number;
  page: number;
  pageSize: number;
  source: MetadataSource;
}

export interface AdminLoginResponse {
  token: string;
  username: string;
  expiresAt: string;
}

export interface AdminSessionInfo {
  authenticated: boolean;
  username?: string;
  expiresAt?: string;
  csrfToken?: string;
}

export interface SteamAuthFlowStatus {
  flowId: string;
  kind: SteamAuthFlowKind;
  state: SteamAuthFlowState;
  challengeUrl?: string;
  qrSvg?: string;
  codePrompt?: string;
  accountName?: string;
  error?: string;
  updatedAt: string;
}

export interface CatalogSteamStatus {
  providerName: string;
  mode: SteamAuthMode;
  connectionState: SteamConnectionState;
  steamId?: string;
  accountName?: string;
  cellId?: number;
  countryCode?: string;
  connectedSince?: string;
  totalRequests: number;
  failedRequests: number;
  lastError?: string;
  activeAuthFlow?: SteamAuthFlowStatus;
}

export interface CatalogSyncConfigDto {
  enableReadThrough: boolean;
  itemTtlSecs: number;
  enableIncrementalSync: boolean;
  incrementalIntervalSecs: number;
  incrementalPagesPerTick: number;
  enableBackfill: boolean;
  backfillIntervalSecs: number;
  backfillBatchSize: number;
}

export interface CatalogSyncStatus {
  config: CatalogSyncConfigDto;
  incrementalRunning: boolean;
  lastIncrementalAt?: string;
  lastIncrementalUpserted: number;
  highWaterTimeUpdated?: number;
  backfillRunning: boolean;
  backfillCursor?: string;
  backfillSyncedTotal: number;
  remoteWorkshopTotal?: number;
  lastSyncError?: string;
}

export interface CatalogStats {
  totalItems: number;
  activeItems: number;
  removedItems: number;
  privateItems: number;
  collectionsCount: number;
  overriddenItems: number;
  revisionsCount: number;
  auditLogsCount: number;
}

export interface CatalogOverviewResponse {
  stats: CatalogStats;
  steam: CatalogSteamStatus;
  sync: CatalogSyncStatus;
  configPath: string | null;
  bindAddr: string;
  dbPath: string;
}

export interface AdminFetchUpstreamResponse {
  items: WorkshopItemRecord[];
  removedIds: string[];
  unavailableIds: string[];
}

export interface AdminItemOverrideRequest {
  status: WorkshopItemStatus;
  title?: string;
  description?: string;
  fileUrl?: string;
  previewUrl?: string;
  dependencies?: WorkshopDependency[];
  reason?: string;
}

export interface ItemRevisionEntry {
  id: number;
  status: WorkshopItemStatus;
  title: string;
  timeUpdated?: number;
  dependencies: WorkshopDependency[];
  source: MetadataSource;
  recordedAt: string;
}

export interface AdminItemDetailResponse {
  item: WorkshopItemRecord;
  isAdminOverride: boolean;
  lastVerifiedAt: string;
  removedAt?: string;
  revisions: ItemRevisionEntry[];
}

export interface AuditLogEntry {
  id: number;
  workshopId?: string;
  actor: string;
  action: string;
  reason: string;
  status?: WorkshopItemStatus;
  timeUpdated?: number;
  createdAt: string;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

let sessionToken: string | null = null;
let csrfToken: string | null = null;

export function setSessionToken(token: string | null): void {
  sessionToken = token;
  csrfToken = null;
}

async function requestJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }
  if (sessionToken && !headers.has('Authorization')) {
    headers.set('Authorization', `Bearer ${sessionToken}`);
  }
  if (csrfToken && init.method && !['GET', 'HEAD', 'OPTIONS'].includes(init.method)) {
    headers.set('X-L4A-CSRF', csrfToken);
  }

  const response = await fetch(path, {
    ...init,
    credentials: 'same-origin',
    headers,
  });

  if (!response.ok) {
    let message = `HTTP ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload.error) {
        message = payload.error;
      }
    } catch {
      // keep fallback HTTP status
    }
    throw new ApiError(response.status, message);
  }

  return (await response.json()) as T;
}

export async function getAdminSession(): Promise<AdminSessionInfo> {
  const session = await requestJson<AdminSessionInfo>('/api/v1/admin/auth/me');
  csrfToken = session.csrfToken ?? null;
  return session;
}

export async function loginAdmin(username: string, password: string): Promise<AdminLoginResponse> {
  const res = await requestJson<AdminLoginResponse>('/api/v1/admin/auth/login', {
    method: 'POST',
    body: JSON.stringify({ username, password }),
  });
  setSessionToken(res.token);
  return res;
}

export async function logoutAdmin(): Promise<void> {
  await requestJson<{ ok: boolean }>('/api/v1/admin/auth/logout', { method: 'POST' });
  setSessionToken(null);
}

export async function changeAdminPassword(
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  await requestJson<{ ok: boolean }>('/api/v1/admin/auth/password', {
    method: 'POST',
    body: JSON.stringify({ currentPassword, newPassword }),
  });
}

export async function getOverview(): Promise<CatalogOverviewResponse> {
  return requestJson<CatalogOverviewResponse>('/api/v1/admin/overview');
}

export async function getSteamStatus(): Promise<CatalogSteamStatus> {
  return requestJson<CatalogSteamStatus>('/api/v1/admin/steam/status');
}

export async function reconnectSteam(): Promise<CatalogSteamStatus> {
  return requestJson<CatalogSteamStatus>('/api/v1/admin/steam/reconnect', { method: 'POST' });
}

export async function startQrAuth(): Promise<SteamAuthFlowStatus> {
  return requestJson<SteamAuthFlowStatus>('/api/v1/admin/steam/auth/qr/start', { method: 'POST' });
}

export async function startCredentialsAuth(
  accountName: string,
  password: string,
): Promise<SteamAuthFlowStatus> {
  return requestJson<SteamAuthFlowStatus>('/api/v1/admin/steam/auth/credentials', {
    method: 'POST',
    body: JSON.stringify({ accountName, password }),
  });
}

export async function submitSteamGuardCode(
  flowId: string,
  code: string,
): Promise<SteamAuthFlowStatus> {
  return requestJson<SteamAuthFlowStatus>('/api/v1/admin/steam/auth/guard', {
    method: 'POST',
    body: JSON.stringify({ flowId, code }),
  });
}

export async function setSteamRefreshToken(
  accountName: string,
  refreshToken: string,
): Promise<CatalogSteamStatus> {
  return requestJson<CatalogSteamStatus>('/api/v1/admin/steam/auth/token', {
    method: 'POST',
    body: JSON.stringify({ accountName, refreshToken }),
  });
}

export async function switchSteamAnonymous(): Promise<CatalogSteamStatus> {
  return requestJson<CatalogSteamStatus>('/api/v1/admin/steam/auth/anonymous', { method: 'POST' });
}

export async function updateSyncConfig(
  config: CatalogSyncConfigDto,
): Promise<CatalogSyncStatus> {
  return requestJson<CatalogSyncStatus>('/api/v1/admin/config/sync', {
    method: 'PATCH',
    body: JSON.stringify(config),
  });
}

export async function triggerIncrementalSync(): Promise<CatalogSyncStatus> {
  return requestJson<CatalogSyncStatus>('/api/v1/admin/sync/trigger', { method: 'POST' });
}

export async function resetBackfillCursor(): Promise<CatalogSyncStatus> {
  return requestJson<CatalogSyncStatus>('/api/v1/admin/sync/reset-cursor', { method: 'POST' });
}

export async function fetchUpstreamItems(ids: string[]): Promise<AdminFetchUpstreamResponse> {
  return requestJson<AdminFetchUpstreamResponse>('/api/v1/admin/sync/fetch', {
    method: 'POST',
    body: JSON.stringify({ ids }),
  });
}

export interface CatalogSearchFilter {
  q?: string;
  tag?: string;
  status?: WorkshopItemStatus | '';
  sort?: string;
  page?: number;
  pageSize?: number;
}

export async function searchCatalogItems(filter: CatalogSearchFilter): Promise<SearchResponse> {
  const params = new URLSearchParams();
  if (filter.q?.trim()) params.set('q', filter.q.trim());
  if (filter.tag?.trim()) params.set('tag', filter.tag.trim());
  if (filter.status) params.set('status', filter.status);
  if (filter.sort) params.set('sort', filter.sort);
  params.set('page', String(filter.page ?? 1));
  params.set('pageSize', String(filter.pageSize ?? 25));
  return requestJson<SearchResponse>(`/api/v1/admin/items/search?${params.toString()}`);
}

export async function getAdminItemDetail(workshopId: string): Promise<AdminItemDetailResponse> {
  return requestJson<AdminItemDetailResponse>(`/api/v1/admin/items/${encodeURIComponent(workshopId)}`);
}

export async function refreshAdminItem(workshopId: string): Promise<AdminItemDetailResponse> {
  return requestJson<AdminItemDetailResponse>(
    `/api/v1/admin/items/${encodeURIComponent(workshopId)}/refresh`,
    { method: 'POST' },
  );
}

export async function overrideCatalogItem(
  workshopId: string,
  payload: AdminItemOverrideRequest,
): Promise<WorkshopItemRecord> {
  return requestJson<WorkshopItemRecord>(
    `/api/v1/admin/items/${encodeURIComponent(workshopId)}/override`,
    {
      method: 'POST',
      body: JSON.stringify(payload),
    },
  );
}

export async function clearItemOverride(workshopId: string): Promise<AdminItemDetailResponse> {
  return requestJson<AdminItemDetailResponse>(
    `/api/v1/admin/items/${encodeURIComponent(workshopId)}/override`,
    { method: 'DELETE' },
  );
}

export async function getAuditLogs(limit = 200): Promise<AuditLogEntry[]> {
  return requestJson<AuditLogEntry[]>(`/api/v1/admin/audit?limit=${limit}`);
}

export function parseWorkshopIdsInput(raw: string): string[] {
  const matches = raw.match(/\b[1-9]\d{2,19}\b/g);
  if (!matches) return [];
  return Array.from(new Set(matches));
}

export function formatBytes(bytes?: number): string {
  if (bytes === undefined || bytes === null) return '—';
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let idx = 0;
  let value = bytes;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx += 1;
  }
  return `${value.toFixed(idx === 0 ? 0 : 1)} ${units[idx]}`;
}

export function formatUnixSeconds(ts?: number): string {
  if (!ts) return '—';
  return new Date(ts * 1000).toLocaleString();
}

export function formatIsoTime(iso?: string): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

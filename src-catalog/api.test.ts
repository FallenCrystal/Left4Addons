import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DEFAULT_FILTER_PARAMS,
  detectFilterPreset,
  formatBytes,
  getAdminSession,
  getPresetFilterParams,
  parseWorkshopIdsInput,
  testFilterPredict,
  updateFilterConfig,
} from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('catalog web console helpers', () => {
  it('extracts deduplicated Workshop IDs from mixed text and Steam URLs', () => {
    const raw = `
      https://steamcommunity.com/sharedfiles/filedetails/?id=2814104912
      2814105100, 2814104912; id=3499001245
    `;
    expect(parseWorkshopIdsInput(raw)).toEqual([
      '2814104912',
      '2814105100',
      '3499001245',
    ]);
  });

  it('formats byte sizes cleanly', () => {
    expect(formatBytes(undefined)).toBe('—');
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1024)).toBe('1.0 KB');
    expect(formatBytes(486_539_264)).toBe('464.0 MB');
  });

  it('maps and detects filter model parameter presets', () => {
    expect(detectFilterPreset(DEFAULT_FILTER_PARAMS)).toBe('recommended');
    const nsfwOnly = getPresetFilterParams('nsfw_only');
    expect(nsfwOnly.goreWeight).toBe(0);
    expect(nsfwOnly.nsfwWeight).toBe(1);
    expect(detectFilterPreset(nsfwOnly)).toBe('nsfw_only');
    expect(
      detectFilterPreset({
        ...DEFAULT_FILTER_PARAMS,
        temperature: 1.75,
      }),
    ).toBe('custom');
  });

  it('sends CSRF token on filter config PATCH and sandbox test POST', async () => {
    const calls: Array<{ path: string; method?: string; csrf: string | null; body?: unknown }> = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (path: string, init?: RequestInit) => {
        const headers = new Headers(init?.headers);
        calls.push({
          path,
          method: init?.method,
          csrf: headers.get('X-L4A-CSRF'),
          body: typeof init?.body === 'string' ? JSON.parse(init.body) : undefined,
        });
        if (path.endsWith('/auth/me')) {
          return Response.json({ authenticated: true, username: 'admin', csrfToken: 'csrf-filter' });
        }
        if (path.endsWith('/admin/config/filter')) {
          return Response.json({
            config: {
              enabled: true,
              backend: 'upstream_http',
              upstreamUrl: 'http://127.0.0.1:8090',
              cacheTtlSecs: 1800,
              params: getPresetFilterParams('nsfw_only'),
            },
            ready: true,
            totalRequests: 0,
            cacheHits: 0,
            failedRequests: 0,
          });
        }
        if (path.endsWith('/admin/filter/test')) {
          return Response.json({
            prob: 0.85,
            goreProb: 0.0,
            nsfwProb: 0.85,
            textProb: 0.85,
          });
        }
        throw new Error(`Unexpected request ${path}`);
      }),
    );

    await getAdminSession();
    const updated = await updateFilterConfig({
      enabled: true,
      backend: 'upstream_http',
      upstreamUrl: 'http://127.0.0.1:8090',
      cacheTtlSecs: 1800,
      params: getPresetFilterParams('nsfw_only'),
    });
    expect(updated.ready).toBe(true);

    const probs = await testFilterPredict({ title: 'Test Mod' });
    expect(probs.prob).toBeCloseTo(0.85);

    expect(calls[1]).toMatchObject({
      path: '/api/v1/admin/config/filter',
      method: 'PATCH',
      csrf: 'csrf-filter',
    });
    expect(calls[2]).toMatchObject({
      path: '/api/v1/admin/filter/test',
      method: 'POST',
      csrf: 'csrf-filter',
      body: { title: 'Test Mod' },
    });
  });
});


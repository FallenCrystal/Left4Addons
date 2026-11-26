import {
  DEFAULT_LLM_TOKEN_ENV_VAR,
  IpcError,
  type ClientFilterConfig,
  type DashboardSnapshot,
  type DeployLocalFilterRequest,
  type FilterCacheClearResult,
  type FilterCommitInfo,
  type FilterDecisionSource,
  type FilterEvaluationBatchResponse,
  type FilterEvaluationItemResult,
  type FilterEvaluationTarget,
  type FilterExclusionRecord,
  type FilterGalleryAggregation,
  type FilterItemVerdict,
  type FilterProbabilities,
  type FilterTokenStatus,
  type LocalFilterDeploymentStatus,
  type UpsertFilterExclusionRequest,
} from '../ipc';
import {
  adjustProbabilities,
  computeFilterPolicySignature,
  createDefaultModelParams,
  filterVerdictExpiresAt,
  isAutomaticFilterTargetEnabled,
  sanitizeModelParams,
  scoreForTarget,
  type FilterPolicySignatureOptions,
} from '../../utils/contentFilter';
import type { PreviewEmit } from './workshop';

const PREVIEW_COMMITS: FilterCommitInfo[] = [
  {
    sha: 'a94f18e2d3104b88c91e776201fa4b928c7d1204',
    shortSha: 'a94f18e',
    subject: 'feat(calibration): refine dual-head NSFW/Gore logit fusion',
    committedAt: '2026-11-22T14:30:00Z',
  },
  {
    sha: '7c3d02b198fe4120a55b63c89d01ee4298ab3319',
    shortSha: '7c3d02b',
    subject: 'perf(onnx): optimize gallery top2_mean multi-image batching',
    committedAt: '2026-11-18T09:15:00Z',
  },
  {
    sha: '5e19a04cc12d49f8831a76002b8e91147f5d6602',
    shortSha: '5e19a04',
    subject: 'fix(text): reduce false positives on red HUD fonts and VScript logs',
    committedAt: '2026-11-10T18:42:00Z',
  },
  {
    sha: '2b88f1109a4e40129c715502e3a11098bc442010',
    shortSha: '2b88f11',
    subject: 'chore: initial Left4Filters ONNX runtime release',
    committedAt: '2026-11-01T12:00:00Z',
  },
];

const NSFW_REGEX =
  /nsfw|r18|18\+|nud[ei]|bikini|lingerie|fetish|erotic|hentai|lewd|adult[\s_-]*mod|色情|成人|暴露|泳装|瑟瑟/i;
const GORE_REGEX =
  /gore|dismember|viscera|mutilat|entrail|bloodbath|decapitat|corpse[\s_-]*pile|血腥|断肢|内脏|猎奇|残肢/i;
const BORDERLINE_NSFW_REGEX = /swimsuit|bunny[\s_-]*suit|cleavage|pinup|性感|清凉/i;
const BORDERLINE_GORE_REGEX = /crimson[\s_-]*blood|splatter|carnage|血浆|重口/i;

function scoreSnippet(text: string): { gore: number; nsfw: number; ov: number } {
  const trimmed = text.trim();
  if (!trimmed) {
    return { gore: 0.008, nsfw: 0.01, ov: 0.018 };
  }
  let nsfw = 0.014;
  let gore = 0.012;
  if (NSFW_REGEX.test(trimmed)) {
    nsfw = 0.89;
  } else if (BORDERLINE_NSFW_REGEX.test(trimmed)) {
    nsfw = 0.36;
  }
  if (GORE_REGEX.test(trimmed)) {
    gore = 0.87;
  } else if (BORDERLINE_GORE_REGEX.test(trimmed)) {
    gore = 0.34;
  }
  const ov = Number((1 - (1 - gore) * (1 - nsfw)).toFixed(6));
  return { gore, nsfw, ov };
}

function aggregateGallery(values: number[], mode: FilterGalleryAggregation): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => b - a);
  if (mode === 'mean') {
    return sorted.reduce((acc, v) => acc + v, 0) / sorted.length;
  }
  if (mode === 'top2_mean') {
    const top = sorted.slice(0, 2);
    return top.reduce((acc, v) => acc + v, 0) / top.length;
  }
  return sorted[0];
}

export function reconcilePreviewExclusions(state: DashboardSnapshot): void {
  const addonsById = new Map(state.addons.map((a) => [a.id, a]));
  for (const excl of state.filterExclusions) {
    const addon = addonsById.get(excl.addonId);
    const activeHash = addon?.installState === 'installed' ? addon.contentHash : undefined;
    excl.currentFingerprint = activeHash;
    if (!excl.bindFingerprint) {
      excl.fingerprintValid = true;
    } else {
      excl.fingerprintValid = Boolean(
        excl.boundFingerprint && activeHash && excl.boundFingerprint === activeHash,
      );
    }
  }
}

export function upsertPreviewFilterExclusion(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  request: UpsertFilterExclusionRequest,
): DashboardSnapshot {
  const addon = state.addons.find((a) => a.id === request.addonId);
  if (!addon) throw new IpcError('not_found', 'Addon not found');
  const now = new Date().toISOString();
  const activeHash = addon.installState === 'installed' ? addon.contentHash : undefined;
  const existingIdx = state.filterExclusions.findIndex((e) => e.addonId === request.addonId);
  const existing = state.filterExclusions[existingIdx];
  if (request.bindFingerprint && !activeHash) {
    throw new IpcError('invalid_input', 'No installed VPK fingerprint');
  }
  const bindFingerprint = request.bindFingerprint ?? existing?.bindFingerprint ?? false;
  const boundFingerprint = request.bindFingerprint === undefined
    ? existing?.boundFingerprint
    : request.bindFingerprint ? activeHash : undefined;

  const nextRecord: FilterExclusionRecord = {
    addonId: request.addonId,
    displayLevel: request.displayLevel,
    bindFingerprint,
    boundFingerprint,
    currentFingerprint: activeHash,
    fingerprintValid: !bindFingerprint || Boolean(boundFingerprint && activeHash === boundFingerprint),
    note: request.note?.trim() || undefined,
    createdAt: existingIdx >= 0 ? state.filterExclusions[existingIdx].createdAt : now,
    updatedAt: now,
  };

  if (existingIdx >= 0) {
    state.filterExclusions[existingIdx] = nextRecord;
  } else {
    state.filterExclusions.unshift(nextRecord);
  }

  emit({ type: 'snapshot_updated', externalChange: false });
  return structuredClone(state);
}

export function deletePreviewFilterExclusion(
  state: DashboardSnapshot,
  emit: PreviewEmit,
  addonId: string,
): DashboardSnapshot {
  state.filterExclusions = state.filterExclusions.filter((e) => e.addonId !== addonId);
  emit({ type: 'snapshot_updated', externalChange: false });
  return structuredClone(state);
}

export class PreviewFilterRuntime {
  private status: LocalFilterDeploymentStatus = this.createInitialStatus();
  private hasEncryptedToken = false;
  private cache = new Map<string, FilterItemVerdict>();
  cacheGeneration = 0;

  private createInitialStatus(): LocalFilterDeploymentStatus {
    return {
      env: {
        gitAvailable: true,
        gitVersion: 'git version 2.45.2 (preview)',
        gitPath: '/usr/bin/git',
        pythonAvailable: true,
        pythonVersion: 'Python 3.12.3 (preview)',
        pythonPath: '/usr/bin/python3',
        venvAvailable: true,
      },
      deployed: true,
      repoDir: '/home/preview/.local/share/Left4Addons/left4filters/repo',
      venvDir: '/home/preview/.local/share/Left4Addons/left4filters/venv',
      serviceState: 'running',
      serviceEndpoint: 'http://127.0.0.1:18791',
      runningPid: 48210,
      currentCommit: structuredClone(PREVIEW_COMMITS[1]),
      remoteLatestCommit: structuredClone(PREVIEW_COMMITS[0]),
      updateAvailable: true,
      recentCommits: structuredClone(PREVIEW_COMMITS),
      stageMessage: 'Ready (browser preview simulation)',
    };
  }

  reset(): void {
    this.status = this.createInitialStatus();
    this.hasEncryptedToken = false;
    this.cache.clear();
    this.cacheGeneration = 0;
  }

  getStatus(): LocalFilterDeploymentStatus {
    return structuredClone(this.status);
  }

  getCachedVerdictsForSnapshot(state: DashboardSnapshot): FilterItemVerdict[] {
    const policySig = computeFilterPolicySignature(state.config.filter, {
      catalogServerUrl: state.config.catalogServerUrl,
      httpsOnly: state.config.httpsOnly,
    });
    const prefix = `${policySig}::`;
    const out: FilterItemVerdict[] = [];
    for (const [cacheKey, verdict] of this.cache) {
      if (cacheKey.startsWith(prefix) && filterVerdictExpiresAt(verdict, state.config.filter.mode) > Date.now()) {
        out.push(structuredClone(verdict));
      }
    }
    return out;
  }

  checkUpdates(emit: PreviewEmit): LocalFilterDeploymentStatus {
    this.status.remoteLatestCommit = structuredClone(PREVIEW_COMMITS[0]);
    this.status.recentCommits = structuredClone(PREVIEW_COMMITS);
    this.status.updateAvailable = Boolean(
      this.status.currentCommit &&
        this.status.remoteLatestCommit &&
        this.status.currentCommit.sha !== this.status.remoteLatestCommit.sha,
    );
    this.status.stageMessage = this.status.updateAvailable
      ? `Update available: ${PREVIEW_COMMITS[0].shortSha}`
      : 'Up to date with remote HEAD';
    emit({ type: 'local_filter_status_changed', status: this.getStatus() });
    return this.getStatus();
  }

  deploy(request: DeployLocalFilterRequest, emit: PreviewEmit): LocalFilterDeploymentStatus {
    const requestedSha = !request.trackLatest ? request.targetCommit?.trim() : undefined;
    let targetCommit = PREVIEW_COMMITS[0];
    if (requestedSha) {
      const matched = PREVIEW_COMMITS.find(
        (c) => c.sha.startsWith(requestedSha) || c.shortSha === requestedSha,
      );
      targetCommit = matched ?? {
        sha: requestedSha.padEnd(40, '0').slice(0, 40),
        shortSha: requestedSha.slice(0, 7),
        subject: `Pinned commit ${requestedSha.slice(0, 7)}`,
        committedAt: new Date().toISOString(),
      };
    }

    this.status.deployed = true;
    this.status.currentCommit = structuredClone(targetCommit);
    this.status.remoteLatestCommit = structuredClone(PREVIEW_COMMITS[0]);
    this.status.updateAvailable = targetCommit.sha !== PREVIEW_COMMITS[0].sha;
    this.status.lastError = undefined;

    if (request.startAfterDeploy) {
      this.status.serviceState = 'running';
      this.status.serviceEndpoint = 'http://127.0.0.1:18791';
      this.status.runningPid = 48210;
      this.status.stageMessage = `Deployed ${targetCommit.shortSha} and started local service`;
    } else {
      this.status.serviceState = 'stopped';
      this.status.serviceEndpoint = undefined;
      this.status.runningPid = undefined;
      this.status.stageMessage = `Deployed ${targetCommit.shortSha} (service stopped)`;
    }

    this.clearCache();
    emit({ type: 'local_filter_status_changed', status: this.getStatus() });
    return this.getStatus();
  }

  startService(emit: PreviewEmit): LocalFilterDeploymentStatus {
    if (!this.status.deployed) {
      throw new IpcError('config', 'Left4Filters is not deployed yet');
    }
    this.status.serviceState = 'running';
    this.status.serviceEndpoint = 'http://127.0.0.1:18791';
    this.status.runningPid = 48210;
    this.status.lastError = undefined;
    this.status.stageMessage = 'Local filter inference service running';
    emit({ type: 'local_filter_status_changed', status: this.getStatus() });
    return this.getStatus();
  }

  stopService(emit: PreviewEmit): LocalFilterDeploymentStatus {
    this.status.serviceState = 'stopped';
    this.status.serviceEndpoint = undefined;
    this.status.runningPid = undefined;
    this.status.stageMessage = 'Local filter inference service stopped';
    emit({ type: 'local_filter_status_changed', status: this.getStatus() });
    return this.getStatus();
  }

  getTokenStatus(config: ClientFilterConfig): FilterTokenStatus {
    return {
      hasEncryptedToken: this.hasEncryptedToken,
      envVarName: config.llmTokenEnvVar?.trim() || DEFAULT_LLM_TOKEN_ENV_VAR,
      envVarPresent: false,
      cacheGeneration: this.cacheGeneration,
    };
  }

  setLlmToken(token: string | null, config: ClientFilterConfig): FilterTokenStatus {
    this.hasEncryptedToken = Boolean(token && token.trim().length > 0);
    this.cacheGeneration += 1;
    for (const [key, verdict] of this.cache) {
      if (verdict.decisionSource === 'llm_flagged_true' || verdict.decisionSource === 'llm_flagged_false') {
        this.cache.delete(key);
      }
    }
    return this.getTokenStatus(config);
  }

  clearCache(): FilterCacheClearResult {
    const count = this.cache.size;
    this.cache.clear();
    this.cacheGeneration += 1;
    return { count, cacheGeneration: this.cacheGeneration };
  }

  evaluateItems(
    targets: FilterEvaluationTarget[],
    config: ClientFilterConfig,
    emit: PreviewEmit,
    policyOptions?: FilterPolicySignatureOptions,
    automatic = false,
  ): FilterEvaluationBatchResponse {
    if (config.mode === 'local' && this.status.serviceState !== 'running') {
      throw new IpcError('config', 'Local Left4Filters service is not running');
    }

    const policySig = computeFilterPolicySignature(config, policyOptions);
    const params =
      config.mode === 'local'
        ? sanitizeModelParams(config.localParams)
        : {
            ...createDefaultModelParams(),
            maxImages: config.mode === 'custom_endpoint'
              ? sanitizeModelParams(config.localParams).maxImages
              : createDefaultModelParams().maxImages,
          };

    const items: FilterEvaluationItemResult[] = [];
    const emittedVerdicts: FilterItemVerdict[] = [];

    for (const target of targets) {
      if (automatic && !isAutomaticFilterTargetEnabled(target.targetKey, config)) {
        items.push({ targetKey: target.targetKey, status: 'failed', error: 'Content filtering is disabled for this scope' });
        continue;
      }
      const cacheKey = `${policySig}::${target.targetKey}`;
      if (!target.bypassCache) {
        const hit = this.cache.get(cacheKey);
        if (hit && hit.complete !== false && (hit.revisionHint ?? '') === (target.revisionHint ?? '') &&
            filterVerdictExpiresAt(hit, config.mode) > Date.now()) {
          const cloned = structuredClone(hit);
          items.push({
            targetKey: target.targetKey,
            status: 'completed',
            verdict: cloned,
            complete: true,
          });
          emittedVerdicts.push(cloned);
          continue;
        }
      }

      const titleScores = params.enableText
        ? scoreSnippet(`${target.title} ${target.tags.join(' ')}`)
        : { gore: 0, nsfw: 0, ov: 0 };
      const descScores =
        params.enableText && target.description
          ? scoreSnippet(target.description)
          : { gore: 0, nsfw: 0, ov: 0 };

      const textGore = Math.max(titleScores.gore, descScores.gore);
      const textNsfw = Math.max(titleScores.nsfw, descScores.nsfw);
      const textOv = Number((1 - (1 - textGore) * (1 - textNsfw)).toFixed(6));

      const evaluatedSlots: { slotIndex: number; src: string }[] = [];
      let hadImageLoadError = false;
      if (params.enableImage) {
        for (let slotIndex = 0; slotIndex < target.imageSources.length; slotIndex += 1) {
          if (evaluatedSlots.length >= params.maxImages) {
            break;
          }
          const src = target.imageSources[slotIndex]?.trim();
          if (!src) continue;
          if (src.startsWith('error://')) {
            hadImageLoadError = true;
            continue;
          }
          evaluatedSlots.push({ slotIndex, src });
        }
      }
      const evaluatedImageIndices = evaluatedSlots.map((s) => s.slotIndex);
      const perImageRaw = evaluatedSlots.map((s) => scoreSnippet(s.src));
      const perImageProbs = perImageRaw.map((s) => s.ov);
      const imageGore = aggregateGallery(
        perImageRaw.map((s) => s.gore),
        params.galleryAggregation,
      );
      const imageNsfw = aggregateGallery(
        perImageRaw.map((s) => s.nsfw),
        params.galleryAggregation,
      );
      const imageOv = aggregateGallery(perImageProbs, params.galleryAggregation);

      const combinedGore = Math.max(textGore, imageGore);
      const combinedNsfw = Math.max(textNsfw, imageNsfw);
      const combinedOv = Number((1 - (1 - combinedGore) * (1 - combinedNsfw)).toFixed(6));

      const rawProbs: FilterProbabilities = {
        prob: combinedOv,
        goreProb: Number(combinedGore.toFixed(6)),
        nsfwProb: Number(combinedNsfw.toFixed(6)),
        textProb: params.enableText ? textOv : undefined,
        textGoreProb: params.enableText ? Number(textGore.toFixed(6)) : undefined,
        textNsfwProb: params.enableText ? Number(textNsfw.toFixed(6)) : undefined,
        titleProb: params.enableText ? titleScores.ov : undefined,
        descriptionProb:
          params.enableText && target.description ? descScores.ov : undefined,
        imageProb: evaluatedSlots.length > 0 ? Number(imageOv.toFixed(6)) : undefined,
        imageGoreProb: evaluatedSlots.length > 0 ? Number(imageGore.toFixed(6)) : undefined,
        imageNsfwProb: evaluatedSlots.length > 0 ? Number(imageNsfw.toFixed(6)) : undefined,
        perImageProbs: evaluatedSlots.length > 0 ? perImageProbs : undefined,
      };

      const probabilities =
        config.mode === 'local' ? adjustProbabilities(params, rawProbs) : rawProbs;
      const evaluatedScore = scoreForTarget(probabilities, config.scoreTarget);

      let flagged = false;
      let decisionSource: FilterDecisionSource = 'model_safe';

      if (config.enableDirectFlag && evaluatedScore >= config.flagThreshold) {
        flagged = true;
        decisionSource = 'model_direct_flag';
      } else if (config.llmEnabled && evaluatedScore >= config.llmThreshold) {
        const combinedText = `${target.title} ${target.description ?? ''} ${evaluatedSlots.map((s) => s.src).join(' ')}`;
        const llmFlag =
          NSFW_REGEX.test(combinedText) ||
          GORE_REGEX.test(combinedText) ||
          BORDERLINE_NSFW_REGEX.test(combinedText) ||
          BORDERLINE_GORE_REGEX.test(combinedText);
        flagged = llmFlag;
        decisionSource = llmFlag ? 'llm_flagged_true' : 'llm_flagged_false';
      } else if (!config.enableDirectFlag && !config.llmEnabled && evaluatedScore >= config.flagThreshold) {
        flagged = true;
        decisionSource = 'model_direct_flag';
      }

      let textSensitive = false;
      let titleSensitive = false;
      let descriptionSensitive = false;
      let imageSensitive = false;
      let sensitiveImageIndices: number[] = [];

      if (flagged) {
        const hasTitle = Boolean(target.title.trim() || target.tags.length > 0);
        const hasDesc = Boolean(target.description?.trim());
        const hasText = hasTitle || hasDesc;
        const nImgs = evaluatedSlots.length;

        if (!config.subdivideModalities) {
          textSensitive = hasText || nImgs === 0;
          titleSensitive = hasTitle || (!hasDesc && textSensitive);
          descriptionSensitive = hasDesc;
          imageSensitive = nImgs > 0;
          sensitiveImageIndices = [...evaluatedImageIndices];
        } else {
          const modThresh = Number.isFinite(config.modalityThreshold)
            ? Math.min(0.95, Math.max(0.05, config.modalityThreshold))
            : 0.3;
          titleSensitive = hasTitle && (probabilities.titleProb ?? 0) >= modThresh;
          descriptionSensitive = hasDesc && (probabilities.descriptionProb ?? 0) >= modThresh;
          textSensitive =
            hasText &&
            ((probabilities.textProb ?? 0) >= modThresh || titleSensitive || descriptionSensitive);

          const adjPerImage = probabilities.perImageProbs ?? [];
          adjPerImage.forEach((p, idx) => {
            const slot = evaluatedSlots[idx];
            if (p >= modThresh && slot) {
              sensitiveImageIndices.push(slot.slotIndex);
            }
          });
          imageSensitive =
            nImgs > 0 &&
            ((probabilities.imageProb ?? 0) >= modThresh ||
              sensitiveImageIndices.length > 0 ||
              !textSensitive);

          if (textSensitive && !titleSensitive && !descriptionSensitive) {
            if (hasTitle && !hasDesc) titleSensitive = true;
            else if (!hasTitle && hasDesc) descriptionSensitive = true;
            else if ((probabilities.titleProb ?? 0) >= (probabilities.descriptionProb ?? 0)) {
              titleSensitive = true;
            } else {
              descriptionSensitive = true;
            }
          }

          if (imageSensitive && sensitiveImageIndices.length === 0 && nImgs > 0) {
            if (adjPerImage.length > 0) {
              let bestIdx = 0;
              adjPerImage.forEach((p, idx) => {
                if (p > (adjPerImage[bestIdx] ?? -1)) bestIdx = idx;
              });
              const slot = evaluatedSlots[bestIdx];
              if (slot) {
                sensitiveImageIndices.push(slot.slotIndex);
              }
            } else {
              sensitiveImageIndices = [...evaluatedImageIndices];
            }
          }
        }
      }

      const complete = !hadImageLoadError;
      const verdict: FilterItemVerdict = {
        targetKey: target.targetKey,
        revisionHint: target.revisionHint,
        flagged,
        decisionSource,
        evaluatedScore,
        probabilities,
        textSensitive,
        titleSensitive,
        descriptionSensitive,
        imageSensitive,
        evaluatedImageIndices,
        sensitiveImageIndices,
        complete,
        evaluatedAt: new Date().toISOString(),
      };

      if (!target.bypassCache && complete) {
        this.cache.set(cacheKey, structuredClone(verdict));
      }
      items.push({
        targetKey: target.targetKey,
        status: 'completed',
        verdict,
        complete,
      });
      if (complete || flagged) {
        emittedVerdicts.push(verdict);
      }
    }

    if (emittedVerdicts.length > 0) {
      emit({
        type: 'filter_verdicts_updated',
        verdicts: structuredClone(emittedVerdicts),
        policySignature: policySig,
        cacheGeneration: this.cacheGeneration,
      });
    }
    return {
      policySignature: policySig,
      cacheGeneration: this.cacheGeneration,
      items,
    };
  }
}

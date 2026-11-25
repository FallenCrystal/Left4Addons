import {
  DEFAULT_LLM_TOKEN_ENV_VAR,
  IpcError,
  type ClientFilterConfig,
  type DashboardSnapshot,
  type DeployLocalFilterRequest,
  type FilterCommitInfo,
  type FilterDecisionSource,
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
  sanitizeModelParams,
  scoreForTarget,
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
    excl.currentFingerprint = addon?.contentHash;
    if (!excl.bindFingerprint) {
      excl.fingerprintValid = true;
    } else {
      excl.fingerprintValid = Boolean(
        excl.boundFingerprint &&
          addon?.contentHash &&
          excl.boundFingerprint === addon.contentHash,
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
  const now = new Date().toISOString();
  const bindFingerprint = Boolean(request.bindFingerprint && addon?.contentHash);
  const boundFingerprint = bindFingerprint ? addon?.contentHash : undefined;
  const existingIdx = state.filterExclusions.findIndex((e) => e.addonId === request.addonId);

  const nextRecord: FilterExclusionRecord = {
    addonId: request.addonId,
    displayLevel: request.displayLevel,
    bindFingerprint,
    boundFingerprint,
    currentFingerprint: addon?.contentHash,
    fingerprintValid: !bindFingerprint || Boolean(boundFingerprint),
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
  }

  getStatus(): LocalFilterDeploymentStatus {
    return structuredClone(this.status);
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

    this.cache.clear();
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
    };
  }

  setLlmToken(token: string | null, config: ClientFilterConfig): FilterTokenStatus {
    this.hasEncryptedToken = Boolean(token && token.trim().length > 0);
    return this.getTokenStatus(config);
  }

  clearCache(): number {
    const count = this.cache.size;
    this.cache.clear();
    return count;
  }

  evaluateItems(
    targets: FilterEvaluationTarget[],
    config: ClientFilterConfig,
    emit: PreviewEmit,
  ): FilterItemVerdict[] {
    if (config.mode === 'local' && this.status.serviceState !== 'running') {
      throw new IpcError('config', 'Local Left4Filters service is not running');
    }

    const policySig = computeFilterPolicySignature(config);
    const params =
      config.mode === 'local'
        ? sanitizeModelParams(config.localParams)
        : createDefaultModelParams();

    const verdicts: FilterItemVerdict[] = [];

    for (const target of targets) {
      const cacheKey = `${policySig}::${target.targetKey}::${target.revisionHint ?? ''}`;
      if (!target.bypassCache) {
        const hit = this.cache.get(cacheKey);
        if (hit) {
          verdicts.push(structuredClone(hit));
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

      const rawSources = params.enableImage
        ? target.imageSources.slice(0, params.maxImages)
        : [];
      const perImageRaw = rawSources.map((src) => scoreSnippet(src));
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
        imageProb: rawSources.length > 0 ? Number(imageOv.toFixed(6)) : undefined,
        imageGoreProb: rawSources.length > 0 ? Number(imageGore.toFixed(6)) : undefined,
        imageNsfwProb: rawSources.length > 0 ? Number(imageNsfw.toFixed(6)) : undefined,
        perImageProbs: rawSources.length > 0 ? perImageProbs : undefined,
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
        const combinedText = `${target.title} ${target.description ?? ''} ${target.imageSources.join(' ')}`;
        const llmFlag =
          NSFW_REGEX.test(combinedText) ||
          GORE_REGEX.test(combinedText) ||
          BORDERLINE_NSFW_REGEX.test(combinedText) ||
          BORDERLINE_GORE_REGEX.test(combinedText);
        flagged = llmFlag;
        decisionSource = llmFlag ? 'llm_flagged_true' : 'llm_flagged_false';
      }

      const modThresh = config.modalityThreshold;
      const titleSensitive = (probabilities.titleProb ?? 0) >= modThresh;
      const descriptionSensitive = (probabilities.descriptionProb ?? 0) >= modThresh;
      const textSensitive =
        (probabilities.textProb ?? 0) >= modThresh || titleSensitive || descriptionSensitive;

      const adjPerImage = probabilities.perImageProbs ?? [];
      const sensitiveImageIndices: number[] = [];
      adjPerImage.forEach((p, idx) => {
        if (p >= modThresh) {
          sensitiveImageIndices.push(idx);
        }
      });
      const imageSensitive =
        (probabilities.imageProb ?? 0) >= modThresh ||
        sensitiveImageIndices.length > 0 ||
        (flagged && !textSensitive);

      const verdict: FilterItemVerdict = {
        targetKey: target.targetKey,
        flagged,
        decisionSource,
        evaluatedScore,
        probabilities,
        textSensitive,
        titleSensitive,
        descriptionSensitive,
        imageSensitive,
        sensitiveImageIndices,
        evaluatedAt: new Date().toISOString(),
      };

      if (!target.bypassCache) {
        this.cache.set(cacheKey, structuredClone(verdict));
      }
      verdicts.push(verdict);
    }

    if (verdicts.length > 0) {
      emit({ type: 'filter_verdicts_updated', verdicts: structuredClone(verdicts) });
    }
    return verdicts;
  }
}

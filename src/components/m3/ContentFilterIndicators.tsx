import { AlertCircle, Eye, LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { CoverFilterMode, FilterEvaluationDisplay } from '../../utils/contentFilter';

export function FilterProgress({ size = 22 }: { size?: number }) {
  const { t } = useTranslation();
  return (
    <span
      className="l4a-filter-progress"
      role="progressbar"
      aria-label={t('filter.evaluatingMedia')}
    >
      <LoaderCircle size={size} className="md-spin" aria-hidden="true" />
    </span>
  );
}

export function FilterCardOverlay({
  state,
  onReveal,
}: {
  state: FilterEvaluationDisplay & { blurWholeCard: boolean };
  onReveal: () => void;
}) {
  const { t } = useTranslation();
  if (!state.blurWholeCard) return null;
  if (state.pendingEvaluation) {
    return (
      <div className="l4a-blur-card-overlay" data-pending="true">
        <FilterProgress />
      </div>
    );
  }

  const showFailureNotice = state.failureDisplay === 'notice';
  return (
    <button
      type="button"
      className="l4a-blur-card-overlay"
      onClick={(event) => {
        event.stopPropagation();
        onReveal();
      }}
    >
      <span className="l4a-blur-card-chip">
        {showFailureNotice ? <AlertCircle size={14} /> : <Eye size={14} />}
        {t(showFailureNotice ? 'filter.evalFailedReveal' : 'filter.blurCardOverlay')}
      </span>
    </button>
  );
}

export function FilterFailureBadge({
  state,
}: {
  state: FilterEvaluationDisplay & { coverMode: CoverFilterMode; blurWholeCard?: boolean };
}) {
  const { t } = useTranslation();
  if (state.blurWholeCard || state.pendingEvaluation || !state.failureDisplay) return null;
  if (state.failureDisplay === 'as_flagged' && state.coverMode !== 'none') return null;

  const showFailureNotice = state.failureDisplay === 'notice';
  return (
    <span className="md-pill" data-tone={showFailureNotice ? 'surface' : 'tertiary'}>
      {showFailureNotice && <AlertCircle size={12} />}
      {t(showFailureNotice ? 'filter.evalFailed' : 'filter.sensitiveContent')}
    </span>
  );
}

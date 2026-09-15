import * as React from 'react';

export type ReportDraftChanges = (id: string, dirty: boolean) => void;

/** A page makes one discard decision for all of its mounted editors. */
export function useDraftChanges(onDirtyChange?: (dirty: boolean) => void) {
  const [editors, setEditors] = React.useState<Record<string, boolean>>({});
  const report = React.useCallback<ReportDraftChanges>((id, dirty) => {
    setEditors((previous) => {
      if (Boolean(previous[id]) === dirty) return previous;
      const next = { ...previous };
      if (dirty) next[id] = true;
      else delete next[id];
      return next;
    });
  }, []);
  const dirty = Object.values(editors).some(Boolean);
  React.useEffect(() => {
    onDirtyChange?.(dirty);
    return () => onDirtyChange?.(false);
  }, [dirty, onDirtyChange]);
  return { dirty, report };
}

export function useReportDraftChanges(report: ReportDraftChanges, id: string, dirty: boolean) {
  React.useEffect(() => {
    report(id, dirty);
    return () => report(id, false);
  }, [report, id, dirty]);
}

import * as React from "react";

type UseCompactComposerInteractionsArgs = {
  compact: boolean;
  onExpand: () => void;
};

export function useCompactComposerInteractions({
  compact,
  onExpand,
}: UseCompactComposerInteractionsArgs) {
  const isToolbarInteractionActiveRef = React.useRef(false);

  const handleToolbarMouseDown = React.useCallback(() => {
    if (compact) onExpand();
    isToolbarInteractionActiveRef.current = true;
    window.setTimeout(() => {
      isToolbarInteractionActiveRef.current = false;
    }, 0);
  }, [compact, onExpand]);

  const shouldIgnoreBlur = React.useCallback(
    () => isToolbarInteractionActiveRef.current,
    [],
  );

  return { handleToolbarMouseDown, shouldIgnoreBlur };
}

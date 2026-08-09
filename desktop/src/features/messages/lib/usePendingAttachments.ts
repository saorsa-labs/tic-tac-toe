import * as React from "react";

import type { BlobDescriptor } from "@/shared/api/tauri";

/** First four hexadecimal characters of a SHA-256 digest for compact labels. */
export function shortHash(sha256: string): string {
  return sha256.slice(0, 4);
}

/**
 * Local composer state for attachments already present in a restored draft or
 * an edited historical message. Native x0x messaging cannot publish new media,
 * so this hook deliberately exposes no picker, upload, paste, or drop actions.
 */
export function usePendingAttachments() {
  const [pendingImeta, setPendingImetaState] = React.useState<BlobDescriptor[]>(
    [],
  );
  const pendingImetaRef = React.useRef(pendingImeta);
  pendingImetaRef.current = pendingImeta;

  const removeAttachment = React.useCallback((url: string) => {
    setPendingImetaState((current) =>
      current.filter((attachment) => attachment.url !== url),
    );
  }, []);

  const setPendingImeta = React.useCallback(
    (action: React.SetStateAction<BlobDescriptor[]>) => {
      setPendingImetaState(action);
    },
    [],
  );

  return React.useMemo(
    () => ({
      pendingImeta,
      pendingImetaRef,
      removeAttachment,
      setPendingImeta,
    }),
    [pendingImeta, removeAttachment, setPendingImeta],
  );
}

export type PendingAttachmentController = ReturnType<
  typeof usePendingAttachments
>;

import * as React from "react";

import type { Channel } from "@/shared/api/types";
import type { CreateChannelInput } from "@/features/sidebar/lib/useCreateChannelForm";
import { useDeferredModalOpen } from "@/shared/ui/deferredModalOpen";

const ChannelBrowserDialog = React.lazy(async () => {
  const module = await import("@/features/channels/ui/ChannelBrowserDialog");
  return { default: module.ChannelBrowserDialog };
});

const ChannelManagementSheet = React.lazy(async () => {
  const module = await import("@/features/channels/ui/ChannelManagementSheet");
  return { default: module.ChannelManagementSheet };
});

export type BrowseDialogType = "stream" | null;

type AppShellOverlaysProps = {
  activeChannel: Channel | null;
  browseDialogType: BrowseDialogType;
  channels: Channel[];
  currentAgentId?: string;
  isChannelManagementOpen: boolean;
  isCreatingBrowseChannel?: boolean;
  onBrowseChannelCreate?: (input: CreateChannelInput) => Promise<void>;
  onBrowseDialogOpenChange: (open: boolean) => void;
  onChannelManagementOpenChange: (open: boolean) => void;
  onDeleteActiveChannel: () => void;
  onSelectChannel: (channelId: string) => void;
};

export function AppShellOverlays({
  activeChannel,
  browseDialogType,
  channels,
  currentAgentId,
  isChannelManagementOpen,
  isCreatingBrowseChannel,
  onBrowseChannelCreate,
  onBrowseDialogOpenChange,
  onChannelManagementOpenChange,
  onDeleteActiveChannel,
  onSelectChannel,
}: AppShellOverlaysProps) {
  const [visibleBrowseDialogType, setVisibleBrowseDialogType] =
    React.useState<BrowseDialogType>(null);
  const { cancelDeferredModalOpen, openNextFrame: openModalNextFrame } =
    useDeferredModalOpen();

  React.useEffect(() => {
    if (browseDialogType === null) {
      cancelDeferredModalOpen();
      setVisibleBrowseDialogType(null);
      return;
    }

    setVisibleBrowseDialogType(null);
    openModalNextFrame(() => {
      setVisibleBrowseDialogType(browseDialogType);
    });
  }, [browseDialogType, cancelDeferredModalOpen, openModalNextFrame]);

  return (
    <>
      {browseDialogType !== null ? (
        <React.Suspense fallback={null}>
          <ChannelBrowserDialog
            channels={channels}
            isCreatingChannel={isCreatingBrowseChannel}
            onCreateChannel={onBrowseChannelCreate}
            onOpenChange={onBrowseDialogOpenChange}
            onSelectChannel={onSelectChannel}
            open={visibleBrowseDialogType !== null}
          />
        </React.Suspense>
      ) : null}

      {isChannelManagementOpen && activeChannel !== null ? (
        <React.Suspense fallback={null}>
          <ChannelManagementSheet
            channel={activeChannel}
            currentAgentId={currentAgentId}
            onDeleted={onDeleteActiveChannel}
            onOpenChange={onChannelManagementOpenChange}
            open={true}
          />
        </React.Suspense>
      ) : null}
    </>
  );
}

import * as React from "react";

export const ChannelPane = React.lazy(async () => {
  const module = await import("@/features/channels/ui/ChannelPane");
  return { default: module.ChannelPane };
});

export const UserProfilePanel = React.lazy(async () => {
  const module = await import("@/features/profile/ui/UserProfilePanel");
  return { default: module.UserProfilePanel };
});

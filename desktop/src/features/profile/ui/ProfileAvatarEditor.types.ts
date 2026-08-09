import type * as React from "react";

export type AvatarMode = "image" | "emoji";
export type AvatarEditorPresentation = "default" | "onboarding-modal";
export type ProfileAvatarEditorProps = {
  avatarUrl: string;
  onUrlChange: (url: string) => void;
  emojiPickerTheme?: "auto" | "dark" | "light";
  emojiPickerThemeVars?: React.CSSProperties;
  onEmojiAvatarChange?: () => void;
  onCustomColorPickerOpenChange?: (isOpen: boolean) => void;
  onModeChange?: (mode: AvatarMode) => void;
  onDone?: () => void;
  donePending?: boolean;
  showEmojiColorControlsWhenEmpty?: boolean;
  disabled?: boolean;
  testIdPrefix?: string;
  modeTabsContainer?: HTMLElement | null;
  presentation?: AvatarEditorPresentation;
};

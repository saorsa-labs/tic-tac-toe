import type { AcpRuntimeCatalogEntry, Profile } from "@/shared/api/types";

export type OnboardingPage = "profile" | "avatar" | "membership-denied";

export type OnboardingActions = {
  complete: () => void;
  skipForNow: () => void;
};

export type OnboardingProfileSeed = {
  profile?: Profile;
};

export type OnboardingProfileValues = {
  avatarUrl: string;
  displayName: string;
};

export type ProfileStepSaveRecovery = {
  canAdvanceWithoutSaving: boolean;
  canSkipForNow: boolean;
  errorMessage: string | null;
};

export type ProfileStepNameState = {
  draftValue: string;
  savedValue: string;
};

export type ProfileStepAvatarState = {
  draftUrl: string;
  savedUrl: string;
};

export type ProfileStepState = {
  avatar: ProfileStepAvatarState;
  isSaving: boolean;
  name: ProfileStepNameState;
  saveRecovery: ProfileStepSaveRecovery;
};

export type ProfileStepActions = {
  advanceWithoutSaving: () => void;
  back?: () => void;
  clearAvatarDraft: () => void;
  skipForNow: () => void;
  submit: () => void;
  updateAvatarUrl: (value: string) => void;
  updateDisplayName: (value: string) => void;
};

export type SetupStepActions = {
  back: () => void;
  next: (readyRuntimeIds: readonly string[]) => void;
};

export type DefaultConfigStepActions = {
  back: () => void;
  complete: () => void;
};

export type SetupStepRuntimeState = {
  errorMessage: string | null;
  isChecking: boolean;
  items: AcpRuntimeCatalogEntry[];
};

export type SetupStepState = {
  runtimeProviders: SetupStepRuntimeState;
};

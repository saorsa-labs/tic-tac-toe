import * as React from "react";
import { setLocalStorageItemWithRecovery } from "@/shared/lib/localStorageQuota";

const STORAGE_KEY = "buzz-community-onboarding-transaction.v2";

export type CommunityOnboardingSource = "first-community" | "add-community";

export type CommunityOnboardingStage =
  | "connecting"
  | "profile"
  | "team-intro"
  | "finalizing"
  | "entering";

export type FirstCommunityPage = "join" | "member";

export type CommunityOnboardingTransaction = {
  id: string;
  source: CommunityOnboardingSource;
  firstCommunityPage?: FirstCommunityPage;
  stage: CommunityOnboardingStage;
  communityName: string;
  reposDir?: string;
  communityId?: string;
  groupId?: string;
  previousCommunityId?: string;
  addedCommunity?: boolean;
  createdAt: string;
  updatedAt: string;
  error?: string;
  acknowledged?: boolean;
};

export type CommunityOnboardingTransactionPatch = Partial<
  Pick<
    CommunityOnboardingTransaction,
    | "stage"
    | "communityId"
    | "groupId"
    | "previousCommunityId"
    | "addedCommunity"
    | "communityName"
    | "error"
    | "acknowledged"
  >
>;

export type StartCommunityOnboardingInput = {
  source: CommunityOnboardingSource;
  firstCommunityPage?: FirstCommunityPage;
  communityName: string;
  reposDir?: string;
  groupId?: string;
};

function isTransaction(
  value: unknown,
): value is CommunityOnboardingTransaction {
  if (!value || typeof value !== "object") return false;
  const transaction = value as Partial<CommunityOnboardingTransaction>;
  return (
    typeof transaction.id === "string" &&
    typeof transaction.communityName === "string" &&
    typeof transaction.createdAt === "string" &&
    typeof transaction.updatedAt === "string" &&
    ["connecting", "profile", "team-intro", "finalizing", "entering"].includes(
      transaction.stage ?? "",
    ) &&
    typeof transaction.groupId === "string"
  );
}

export function loadCommunityOnboardingTransaction(
  storage: Storage = localStorage,
): CommunityOnboardingTransaction | null {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    return isTransaction(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export function saveCommunityOnboardingTransaction(
  transaction: CommunityOnboardingTransaction,
  storage: Storage = localStorage,
): void {
  const serialized = JSON.stringify(transaction);
  if (typeof localStorage !== "undefined" && storage === localStorage) {
    setLocalStorageItemWithRecovery(STORAGE_KEY, serialized);
  } else {
    storage.setItem(STORAGE_KEY, serialized);
  }
}

export function clearCommunityOnboardingTransaction(
  storage: Storage = localStorage,
): void {
  storage.removeItem(STORAGE_KEY);
}

export function startCommunityOnboarding(
  input: StartCommunityOnboardingInput,
  storage: Storage = localStorage,
  now = new Date(),
): CommunityOnboardingTransaction {
  const existing = loadCommunityOnboardingTransaction(storage);
  const scope = input.groupId;
  const existingScope = existing?.groupId;
  if (existing && scope === existingScope) {
    const updated = {
      ...existing,
      firstCommunityPage:
        input.firstCommunityPage ?? existing.firstCommunityPage,
      communityName: input.communityName.trim() || existing.communityName,
      reposDir: input.reposDir ?? existing.reposDir,
      updatedAt: now.toISOString(),
      error: undefined,
      acknowledged: undefined,
    };
    saveCommunityOnboardingTransaction(updated, storage);
    return updated;
  }

  const timestamp = now.toISOString();
  const transaction: CommunityOnboardingTransaction = {
    id: crypto.randomUUID(),
    source: input.source,
    firstCommunityPage: input.firstCommunityPage,
    stage: "connecting",
    communityName: input.communityName.trim(),
    groupId: input.groupId,
    reposDir: input.reposDir,
    createdAt: timestamp,
    updatedAt: timestamp,
  };
  saveCommunityOnboardingTransaction(transaction, storage);
  return transaction;
}

export function updateCommunityOnboardingTransaction(
  transaction: CommunityOnboardingTransaction,
  patch: CommunityOnboardingTransactionPatch,
  storage: Storage = localStorage,
  now = new Date(),
): CommunityOnboardingTransaction {
  const updated = { ...transaction, ...patch, updatedAt: now.toISOString() };
  saveCommunityOnboardingTransaction(updated, storage);
  return updated;
}

export function updateCurrentCommunityOnboardingTransaction(
  current: CommunityOnboardingTransaction | null,
  patch: CommunityOnboardingTransactionPatch,
  expectedId: string | undefined,
  storage: Storage = localStorage,
  now = new Date(),
): CommunityOnboardingTransaction | null {
  if (!current || (expectedId && current.id !== expectedId)) return current;
  return updateCommunityOnboardingTransaction(current, patch, storage, now);
}

type CommunityOnboardingContextValue = {
  transaction: CommunityOnboardingTransaction | null;
  start: (input: StartCommunityOnboardingInput) => boolean;
  update: (
    patch: CommunityOnboardingTransactionPatch,
    expectedId?: string,
  ) => void;
  clear: () => void;
};

const CommunityOnboardingContext =
  React.createContext<CommunityOnboardingContextValue | null>(null);

export function CommunityOnboardingProvider({
  children,
}: {
  children: React.ReactNode;
}) {
  const [transaction, setTransaction] = React.useState(
    loadCommunityOnboardingTransaction,
  );
  const start = React.useCallback(
    (input: StartCommunityOnboardingInput) => {
      const scope = input.groupId;
      const activeScope = transaction?.groupId;
      if (transaction && scope !== activeScope) return false;
      setTransaction(startCommunityOnboarding(input));
      return true;
    },
    [transaction],
  );
  const update = React.useCallback(
    (patch: CommunityOnboardingTransactionPatch, expectedId?: string) => {
      setTransaction((current) =>
        updateCurrentCommunityOnboardingTransaction(current, patch, expectedId),
      );
    },
    [],
  );
  const clear = React.useCallback(() => {
    clearCommunityOnboardingTransaction();
    setTransaction(null);
  }, []);
  const value = React.useMemo(
    () => ({ transaction, start, update, clear }),
    [clear, start, transaction, update],
  );
  return (
    <CommunityOnboardingContext.Provider value={value}>
      {children}
    </CommunityOnboardingContext.Provider>
  );
}

export function useCommunityOnboarding() {
  const context = React.useContext(CommunityOnboardingContext);
  if (!context) {
    throw new Error(
      "useCommunityOnboarding must be used within CommunityOnboardingProvider",
    );
  }
  return context;
}

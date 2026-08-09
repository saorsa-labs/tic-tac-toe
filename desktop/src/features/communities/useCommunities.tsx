import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactNode } from "react";

import type { Community } from "./types";
import {
  leaveNativeCommunity,
  listNativeCommunities,
} from "./nativeCommunityApi";
import { x0xUpdateGroup } from "@/shared/api/tauriNativeX0x";
import {
  clearCommunityStorage,
  loadActiveCommunityId,
  saveActiveCommunityId,
} from "./communityStorage";
import { clearSavedCommunitySnapshot } from "@/features/agents/activeAgentTurnsStore";
import {
  clearCommunityDestinations,
  removeCommunityDestination,
} from "./communityNavigationStorage";

export type UpdateCommunityResult =
  | { kind: "updated"; requiresReinit: boolean }
  | { kind: "unchanged" }
  | { kind: "not-found" };

export function resolveCommunityUpdateResult(
  communities: Community[],
  activeId: string | null,
  id: string,
  updates: Partial<Pick<Community, "name" | "reposDir">>,
): UpdateCommunityResult {
  const current = communities.find((community) => community.id === id);
  if (!current) return { kind: "not-found" };

  const hasChange =
    (updates.name !== undefined && updates.name !== current.name) ||
    (updates.reposDir !== undefined && updates.reposDir !== current.reposDir);
  if (!hasChange) return { kind: "unchanged" };

  return {
    kind: "updated",
    requiresReinit:
      id === activeId &&
      updates.reposDir !== undefined &&
      updates.reposDir !== current.reposDir,
  };
}

export function applyCommunitiesOrder(
  communities: Community[],
  orderedIds: string[],
): Community[] {
  const byId = new Map(
    communities.map((community) => [community.id, community]),
  );
  const seen = new Set<string>();
  const reordered: Community[] = [];

  for (const id of orderedIds) {
    const community = byId.get(id);
    if (community && !seen.has(id)) {
      reordered.push(community);
      seen.add(id);
    }
  }
  for (const community of communities) {
    if (!seen.has(community.id)) reordered.push(community);
  }
  return reordered;
}

export type UseCommunitiesReturn = {
  communities: Community[];
  activeCommunity: Community | null;
  reinitKey: number;
  addCommunity: (community: Community) => string;
  clearCommunities: () => void;
  removeCommunity: (id: string) => void;
  switchCommunity: (id: string) => void;
  reconnectCommunity: () => void;
  updateCommunity: (
    id: string,
    updates: Partial<Pick<Community, "name" | "reposDir">>,
  ) => Promise<UpdateCommunityResult>;
  reorderCommunities: (orderedIds: string[]) => void;
};

const CommunitiesContext = createContext<UseCommunitiesReturn | null>(null);

export function CommunitiesProvider({ children }: { children: ReactNode }) {
  const value = useCommunitiesInternal();
  return (
    <CommunitiesContext.Provider value={value}>
      {children}
    </CommunitiesContext.Provider>
  );
}

export function useCommunities(): UseCommunitiesReturn {
  const context = useContext(CommunitiesContext);
  if (!context) {
    throw new Error("useCommunities must be used within a CommunitiesProvider");
  }
  return context;
}

function useCommunitiesInternal(): UseCommunitiesReturn {
  const [communities, setCommunities] = useState<Community[]>([]);
  const [activeId, setActiveId] = useState<string | null>(
    loadActiveCommunityId,
  );
  const [reinitKey, setReinitKey] = useState(0);
  const communitiesRef = useRef(communities);
  communitiesRef.current = communities;

  useEffect(() => {
    let cancelled = false;
    void listNativeCommunities()
      .then((groups) => {
        if (cancelled) return;
        setCommunities(groups);
        const preferredId = loadActiveCommunityId();
        if (
          groups.length > 0 &&
          !groups.some((group) => group.id === preferredId)
        ) {
          saveActiveCommunityId(groups[0].id);
          setActiveId(groups[0].id);
        }
      })
      .catch(() => {
        if (!cancelled) setCommunities([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const activeCommunity = useMemo(
    () =>
      communities.find((community) => community.id === activeId) ??
      communities[0] ??
      null,
    [communities, activeId],
  );

  const addCommunity = useCallback((community: Community): string => {
    const existing = communitiesRef.current.find(
      (candidate) => candidate.groupId === community.groupId,
    );
    const resolvedId = existing?.id ?? community.id;
    setCommunities((current) => {
      const duplicate = current.find(
        (candidate) => candidate.groupId === community.groupId,
      );
      if (!duplicate) return [...current, community];
      return current.map((candidate) =>
        candidate.id === duplicate.id
          ? { ...candidate, ...community }
          : candidate,
      );
    });
    return resolvedId;
  }, []);

  const clearCommunities = useCallback(() => {
    clearCommunityStorage();
    clearCommunityDestinations();
    setCommunities([]);
    setActiveId(null);
  }, []);

  const removeCommunity = useCallback(
    (id: string) => {
      const target = communitiesRef.current.find(
        (community) => community.id === id,
      );
      if (!target) return;
      void leaveNativeCommunity(target.groupId)
        .then(() => listNativeCommunities())
        .then((groups) => {
          setCommunities(groups);
          clearSavedCommunitySnapshot(id);
          removeCommunityDestination(id);
          if (activeId === id) {
            const nextId = groups[0]?.id ?? null;
            if (nextId) saveActiveCommunityId(nextId);
            else clearCommunityStorage();
            setActiveId(nextId);
          }
        })
        .catch(() => {
          void listNativeCommunities().then(setCommunities);
        });
    },
    [activeId],
  );

  const switchCommunity = useCallback(
    (id: string) => {
      if (id === activeId) return;
      saveActiveCommunityId(id);
      setActiveId(id);
    },
    [activeId],
  );

  const reconnectCommunity = useCallback(() => {
    setReinitKey((value) => value + 1);
  }, []);

  const updateCommunity = useCallback(
    async (
      id: string,
      updates: Partial<Pick<Community, "name" | "reposDir">>,
    ): Promise<UpdateCommunityResult> => {
      const result = resolveCommunityUpdateResult(
        communitiesRef.current,
        activeId,
        id,
        updates,
      );
      if (result.kind !== "updated") return result;

      const current = communitiesRef.current.find(
        (community) => community.id === id,
      );
      if (!current) return { kind: "not-found" };
      if (updates.name !== undefined && updates.name !== current.name) {
        await x0xUpdateGroup({ groupId: current.groupId, name: updates.name });
      }
      setCommunities((items) =>
        items.map((community) =>
          community.id === id ? { ...community, ...updates } : community,
        ),
      );
      if (result.requiresReinit) setReinitKey((value) => value + 1);
      return result;
    },
    [activeId],
  );

  const reorderCommunities = useCallback((orderedIds: string[]) => {
    setCommunities((current) => applyCommunitiesOrder(current, orderedIds));
  }, []);

  return {
    communities,
    activeCommunity,
    reinitKey,
    addCommunity,
    clearCommunities,
    removeCommunity,
    switchCommunity,
    reconnectCommunity,
    updateCommunity,
    reorderCommunities,
  };
}

import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import type {
  InstantiateCompanyTemplateInput,
  SymphonyApproval,
  SymphonyRunStatus,
} from "@/shared/api/symphonyTypes";
import {
  approveSymphonyTask,
  cancelCompanyRun,
  denySymphonyTask,
  getSymphonyDaemonStatus,
  getSymphonyRun,
  instantiateCompanyTemplate,
  listCompanyTemplates,
  listSymphonyApprovals,
  listSymphonyProofs,
  listSymphonyTasks,
  listSymphonyWorkers,
  subscribeSymphonyEvents,
} from "@/shared/api/tauriSymphony";

export const symphonyDaemonStatusQueryKey = ["symphony-daemon-status"] as const;
export const symphonyTasksQueryKey = ["symphony-tasks"] as const;
export const symphonyWorkersQueryKey = ["symphony-workers"] as const;
export const symphonyRunQueryKey = (runId: string) =>
  ["symphony-run", runId] as const;
export const symphonyApprovalsQueryKey = (runId: string) =>
  ["symphony-approvals", runId] as const;
export const symphonyProofsQueryKey = (runId: string) =>
  ["symphony-proofs", runId] as const;
export const companyTemplatesQueryKey = ["company-templates"] as const;

function isActive(status: SymphonyRunStatus): boolean {
  return (
    status === "pending" ||
    status === "running" ||
    status === "waiting_approval"
  );
}

export function useSymphonyDaemonStatusQuery() {
  return useQuery({
    queryKey: symphonyDaemonStatusQueryKey,
    queryFn: getSymphonyDaemonStatus,
    refetchInterval: 5_000,
  });
}

function useDaemonAvailable(): boolean {
  return useSymphonyDaemonStatusQuery().data?.available ?? false;
}

export function useSymphonyTasksQuery() {
  const available = useDaemonAvailable();
  return useQuery({
    queryKey: symphonyTasksQueryKey,
    queryFn: listSymphonyTasks,
    enabled: available,
    staleTime: 5_000,
  });
}

export function useSymphonyWorkersQuery() {
  const available = useDaemonAvailable();
  return useQuery({
    queryKey: symphonyWorkersQueryKey,
    queryFn: listSymphonyWorkers,
    enabled: available,
    staleTime: 5_000,
  });
}

export function useSymphonyRunQuery(runId: string | null) {
  const available = useDaemonAvailable();
  return useQuery({
    queryKey: symphonyRunQueryKey(runId ?? ""),
    queryFn: () => getSymphonyRun(runId ?? ""),
    enabled: available && runId !== null,
    refetchInterval: (query) =>
      query.state.data && isActive(query.state.data.status) ? 2_000 : false,
  });
}

export function useSymphonyApprovalsQuery(runId: string | null) {
  const available = useDaemonAvailable();
  return useQuery({
    queryKey: symphonyApprovalsQueryKey(runId ?? ""),
    queryFn: () => listSymphonyApprovals(runId ?? ""),
    enabled: available && runId !== null,
    refetchInterval: 5_000,
  });
}

export function useSymphonyProofsQuery(runId: string | null) {
  const available = useDaemonAvailable();
  return useQuery({
    queryKey: symphonyProofsQueryKey(runId ?? ""),
    queryFn: () => listSymphonyProofs(runId ?? ""),
    enabled: available && runId !== null,
    staleTime: 3_000,
  });
}

export function useCompanyTemplatesQuery() {
  return useQuery({
    queryKey: companyTemplatesQueryKey,
    queryFn: listCompanyTemplates,
    staleTime: 60_000,
  });
}

export function useInstantiateCompanyTemplateMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (
      input: { templateId: string } & InstantiateCompanyTemplateInput,
    ) =>
      instantiateCompanyTemplate(input.templateId, {
        displayName: input.displayName,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: symphonyDaemonStatusQueryKey,
      });
      void queryClient.invalidateQueries({ queryKey: symphonyTasksQueryKey });
      void queryClient.invalidateQueries({ queryKey: symphonyWorkersQueryKey });
    },
  });
}

export function useApprovalDecisionMutation(verdict: "approve" | "deny") {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (approval: SymphonyApproval) =>
      verdict === "approve"
        ? approveSymphonyTask(approval)
        : denySymphonyTask(approval),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["symphony-approvals"] });
      void queryClient.invalidateQueries({ queryKey: symphonyTasksQueryKey });
    },
  });
}

export function useCancelCompanyRunMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: cancelCompanyRun,
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: symphonyDaemonStatusQueryKey,
      });
      void queryClient.invalidateQueries({ queryKey: symphonyTasksQueryKey });
      void queryClient.invalidateQueries({ queryKey: symphonyWorkersQueryKey });
    },
  });
}

/** Mount once at the Company root to turn native SSE frames into cache refreshes. */
export function useSymphonyLiveEventCache(): void {
  const queryClient = useQueryClient();
  const available = useDaemonAvailable();
  useEffect(() => {
    if (!available) return;
    void subscribeSymphonyEvents();
    const unlisten = listen<{ event: string; data: string }>(
      "symphony-event",
      () => {
        void queryClient.invalidateQueries({ queryKey: symphonyTasksQueryKey });
        void queryClient.invalidateQueries({
          queryKey: symphonyWorkersQueryKey,
        });
        void queryClient.invalidateQueries({ queryKey: ["symphony-run"] });
        void queryClient.invalidateQueries({
          queryKey: ["symphony-approvals"],
        });
        void queryClient.invalidateQueries({ queryKey: ["symphony-proofs"] });
      },
    );
    return () => void unlisten.then((dispose) => dispose());
  }, [available, queryClient]);
}

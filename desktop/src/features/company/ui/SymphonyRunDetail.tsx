import { AlertCircle, CheckCircle2, Clock, FileText } from "lucide-react";
import {
  useApprovalDecisionMutation,
  useCancelCompanyRunMutation,
  useSymphonyApprovalsQuery,
  useSymphonyProofsQuery,
  useSymphonyRunQuery,
} from "@/features/symphony/hooks";
import type { SymphonyRunStatus } from "@/shared/api/symphonyTypes";
import { Badge } from "@/shared/ui/badge";
import { Button } from "@/shared/ui/button";
import { Card } from "@/shared/ui/card";
import { Skeleton } from "@/shared/ui/skeleton";

const RUN_STATUS_LABEL: Record<SymphonyRunStatus, string> = {
  pending: "Pending",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  waiting_approval: "Waiting for approval",
};

type Props = {
  instanceId: string | null;
  runId: string;
  onLeave: () => void;
};

export function SymphonyRunDetail({ instanceId, runId, onLeave }: Props) {
  const runQuery = useSymphonyRunQuery(runId);
  const approvalsQuery = useSymphonyApprovalsQuery(runId);
  const proofsQuery = useSymphonyProofsQuery(runId);
  const approve = useApprovalDecisionMutation("approve");
  const deny = useApprovalDecisionMutation("deny");
  const cancel = useCancelCompanyRunMutation();
  const run = runQuery.data;

  return (
    <div
      className="flex h-full min-h-0 flex-col gap-4 overflow-y-auto p-4"
      data-testid="symphony-run-detail"
    >
      <div className="flex items-center justify-between gap-2">
        <div>
          <h2 className="text-base font-semibold">Company run</h2>
          <p className="text-xs text-muted-foreground">{runId}</p>
        </div>
        <div className="flex gap-2">
          {instanceId ? (
            <Button
              disabled={cancel.isPending}
              onClick={() =>
                cancel.mutate({ instanceId, runId }, { onSuccess: onLeave })
              }
              size="sm"
              variant="destructive"
            >
              {cancel.isPending ? "Cancelling…" : "Cancel"}
            </Button>
          ) : null}
          <Button onClick={onLeave} size="sm" variant="outline">
            Leave
          </Button>
        </div>
      </div>
      {cancel.isError ? (
        <p className="flex items-center gap-1.5 text-xs text-red-400">
          <AlertCircle className="h-3.5 w-3.5" /> Failed to cancel:{" "}
          {cancel.error instanceof Error
            ? cancel.error.message
            : String(cancel.error)}
        </p>
      ) : null}

      {runQuery.isLoading ? (
        <Skeleton className="h-16 w-full" />
      ) : run ? (
        <Card className="flex items-center justify-between p-3">
          <span className="text-sm text-muted-foreground">Status</span>
          <Badge variant={run.status === "completed" ? "success" : "info"}>
            {RUN_STATUS_LABEL[run.status]}
          </Badge>
        </Card>
      ) : (
        <p className="text-sm text-muted-foreground">Run not found.</p>
      )}

      <section className="space-y-2">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <Clock className="h-3.5 w-3.5" /> Approvals
        </h3>
        {approvalsQuery.isLoading ? <Skeleton className="h-10 w-full" /> : null}
        {approvalsQuery.data?.map((approval) => (
          <Card className="space-y-2 p-3" key={approval.token}>
            <p className="text-sm font-medium">{approval.title}</p>
            <p className="text-xs text-muted-foreground">
              Signer {approval.signerAgentId}
            </p>
            <div className="flex gap-2">
              <Button
                disabled={approve.isPending || deny.isPending}
                onClick={() => approve.mutate(approval)}
                size="sm"
              >
                Approve
              </Button>
              <Button
                disabled={approve.isPending || deny.isPending}
                onClick={() => deny.mutate(approval)}
                size="sm"
                variant="outline"
              >
                Deny
              </Button>
            </div>
          </Card>
        ))}
        {!approvalsQuery.isLoading && approvalsQuery.data?.length === 0 ? (
          <p className="text-xs text-muted-foreground">No approval gates.</p>
        ) : null}
      </section>

      <section className="space-y-2">
        <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          <CheckCircle2 className="h-3.5 w-3.5" /> Proof stream
        </h3>
        {proofsQuery.isLoading ? <Skeleton className="h-10 w-full" /> : null}
        {proofsQuery.data?.map((proof) => (
          <Card className="space-y-1 p-3" key={proof.id}>
            <div className="flex items-center gap-2 text-xs font-medium">
              <FileText className="h-3.5 w-3.5" />
              {proof.kind}
            </div>
            <pre className="max-h-52 overflow-auto whitespace-pre-wrap text-xs text-muted-foreground">
              {proof.summary}
            </pre>
          </Card>
        ))}
        {!proofsQuery.isLoading && proofsQuery.data?.length === 0 ? (
          <p className="text-xs text-muted-foreground">No proofs yet.</p>
        ) : null}
      </section>

      {runQuery.isError ? (
        <p className="flex items-center gap-1.5 text-xs text-red-400">
          <AlertCircle className="h-3.5 w-3.5" /> Failed to load run detail:{" "}
          {runQuery.error instanceof Error
            ? runQuery.error.message
            : String(runQuery.error)}
        </p>
      ) : null}
    </div>
  );
}

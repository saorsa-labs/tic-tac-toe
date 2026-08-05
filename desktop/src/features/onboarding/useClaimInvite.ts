import * as React from "react";

import { useCommunityOnboarding } from "@/features/onboarding/communityOnboarding";
import {
  inviteErrorMessage,
  isInviteExpiredError,
} from "@/shared/api/inviteHelpers";
import { joinNativeCommunity } from "@/features/communities/nativeCommunityApi";

/**
 * Drive the `claiming` stage after machine onboarding completes: join the
 * native x0xd group with the final AgentId, then advance to `connecting`.
 * Completion is fenced by transaction ID so cancelling or replacing the
 * transaction while the request is pending cannot mutate the replacement.
 *
 * The error guard keeps a failed claim parked on the caller's Retry
 * affordance — without it the effect refires on the error-bearing transaction
 * and re-claims in a loop.
 */
export function useClaimInvite() {
  const { transaction, update } = useCommunityOnboarding();
  const [isPending, setIsPending] = React.useState(false);

  React.useEffect(() => {
    if (transaction?.stage !== "claiming" || transaction.error || isPending) {
      return;
    }
    setIsPending(true);
    void joinNativeCommunity({ invite: transaction.inviteCode ?? "" })
      .then((group) => {
        update(
          {
            stage: "connecting",
            groupId: group.groupId,
            communityName: group.name,
            error: undefined,
          },
          transaction.id,
        );
      })
      .catch((error: unknown) =>
        update(
          {
            error: isInviteExpiredError(error)
              ? "This invite code has expired — ask for a new one."
              : inviteErrorMessage(error),
          },
          transaction.id,
        ),
      )
      .finally(() => setIsPending(false));
  }, [isPending, transaction, update]);
}

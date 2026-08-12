import { isAgentIdentityInManagedList } from "@/features/agents/lib/agentAutocompleteEligibility";
import type { NativeContact } from "@/features/profile/nativeSocialApi";
import { isEstablishedNativeContact } from "@/features/profile/nativeSocialApi";
import { normalizePubkey } from "@/shared/lib/pubkey";

export function getEstablishedNativeContactPubkeys(
  contacts: readonly NativeContact[] | undefined,
) {
  return new Set(
    (contacts ?? [])
      .filter(isEstablishedNativeContact)
      .map((contact) => normalizePubkey(contact.agentId)),
  );
}

export function isEligibleMemberAddCandidate(
  candidate: { isAgent?: boolean; pubkey: string },
  managedAgentPubkeys: ReadonlySet<string>,
  establishedNativeContactPubkeys: ReadonlySet<string>,
) {
  return (
    isAgentIdentityInManagedList(candidate, managedAgentPubkeys) ||
    establishedNativeContactPubkeys.has(normalizePubkey(candidate.pubkey))
  );
}

export function isEstablishedNativeContactCandidate(
  candidate: { pubkey: string },
  establishedNativeContactPubkeys: ReadonlySet<string>,
) {
  return establishedNativeContactPubkeys.has(normalizePubkey(candidate.pubkey));
}

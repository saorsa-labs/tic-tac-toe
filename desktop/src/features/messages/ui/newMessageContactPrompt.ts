import {
  type AddedNativeContact,
  classifyNativeContactInput,
  isEstablishedNativeContact,
  type NativeContact,
  type NativeContactInput,
} from "@/features/profile/nativeSocialApi";

export type NewMessageContactPrompt =
  | {
      kind: "action";
      actionLabel: string;
      description: string;
      input: Extract<NativeContactInput, { kind: "agentCard" | "agentId" }>;
    }
  | { kind: "explanation"; message: string }
  | { kind: "empty"; message: string };

/** Choose the recipient-picker empty state without losing native contact input. */
export function getNewMessageContactPrompt(
  query: string,
  nativeContacts: readonly NativeContact[] = [],
): NewMessageContactPrompt {
  const trimmedQuery = query.trim();
  if (!trimmedQuery) {
    return {
      kind: "empty",
      message: "No people or agents available to message.",
    };
  }

  const input = classifyNativeContactInput(trimmedQuery);
  if (input.kind === "agentCard") {
    return {
      kind: "action",
      actionLabel: "Import signed agent card",
      description:
        "Save this verified x0x contact and try to connect directly.",
      input,
    };
  }
  if (input.kind === "agentId") {
    return {
      kind: "action",
      actionLabel: "Add contact by Agent ID",
      description: "Save this exact x0x Agent ID and try to connect directly.",
      input,
    };
  }
  if (input.kind === "fourWords") {
    const normalizedWords = input.words.join(" ");
    const contactMatches = nativeContacts.filter(
      (contact) =>
        contact.label
          ?.trim()
          .toLowerCase()
          .split(/[\s-]+/)
          .filter(Boolean)
          .join(" ") === normalizedWords,
    );
    const discoveredContact = contactMatches[0];
    if (
      contactMatches.length === 1 &&
      discoveredContact &&
      !isEstablishedNativeContact(discoveredContact)
    ) {
      return {
        kind: "action",
        actionLabel: "Add discovered contact",
        description:
          "One discovered x0x contact matches these words. Confirm it to save the full Agent ID as a known contact.",
        input: { kind: "agentId", agentId: discoveredContact.agentId },
      };
    }
    return {
      kind: "explanation",
      message:
        "Four-word identities are easy-to-read prefixes, not unique addresses. Ask for a signed agent card or the full 64-character Agent ID.",
    };
  }

  return { kind: "empty", message: "No matching users." };
}

/**
 * Exact native contact actions win only when their full Agent ID is not already
 * present in the trust-filtered selectable directory.
 */
export function shouldPreferNewMessageContactPrompt(
  prompt: NewMessageContactPrompt,
  selectableAgentIds: Iterable<string>,
): boolean {
  if (prompt.kind !== "action") {
    return false;
  }
  if (prompt.input.kind === "agentCard") {
    return true;
  }

  const targetAgentId = prompt.input.agentId.toLowerCase();
  return ![...selectableAgentIds].some(
    (agentId) => agentId.trim().toLowerCase() === targetAgentId,
  );
}

/** Keep contact persistence success explicit even when live dialing is offline. */
export function getAddedNativeContactStatus(contact: AddedNativeContact) {
  const savedOnly =
    contact.connectionError !== null ||
    contact.connectionOutcome === "Unreachable" ||
    contact.connectionOutcome === "NotFound";
  return savedOnly
    ? "Contact saved. They are offline or not reachable yet; presence will update automatically."
    : "Contact saved and connected.";
}

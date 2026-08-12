import {
  type AddedNativeContact,
  classifyNativeContactInput,
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
    return {
      kind: "explanation",
      message:
        "Four-word identities are easy-to-read prefixes, not unique addresses. Ask for a signed agent card or the full 64-character Agent ID.",
    };
  }

  return { kind: "empty", message: "No matching users." };
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

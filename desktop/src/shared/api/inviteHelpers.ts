export const INVITE_EXPIRED_ERROR = "invite_expired";

export function inviteErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : `${error}`;
}

export function isInviteExpiredError(error: unknown): boolean {
  return inviteErrorMessage(error) === INVITE_EXPIRED_ERROR;
}

import * as React from "react";
import type { QueryClient } from "@tanstack/react-query";

import { getIdentity, recoverLostIdentity } from "@/shared/api/tauriIdentity";
import type { Identity } from "@/shared/api/types";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";
import { TicTacToeMark } from "@/shared/ui/TicTacToeMark";
import { DefaultConfigStep } from "./DefaultConfigStep";
import {
  ONBOARDING_LANDING_CTA_CLASS,
  OnboardingChrome,
} from "./OnboardingChrome";
import { OnboardingFooterProvider } from "./OnboardingFooter";
import { OnboardingSlideTransition } from "./OnboardingSlideTransition";
import { SetupStep } from "./SetupStep";

export type MachineOnboardingPage = "identity" | "setup" | "config";

export function MachineOnboardingFlow({
  complete,
  identityLost,
  initialPage,
  queryClient,
}: {
  complete: (pubkey?: string) => void;
  identityLost: boolean;
  initialPage?: MachineOnboardingPage;
  queryClient: QueryClient;
}) {
  const [page, setPage] = React.useState<MachineOnboardingPage>(
    initialPage ?? "identity",
  );
  const [error, setError] = React.useState<string | null>(null);
  const [isPending, setIsPending] = React.useState(false);
  const [identity, setIdentity] = React.useState<Identity | null>(null);
  const [readyRuntimeIds, setReadyRuntimeIds] = React.useState<string[]>([]);
  const handleReadyRuntimeIdsChange = React.useCallback(
    (runtimeIds: readonly string[]) => {
      setReadyRuntimeIds(Array.from(new Set(runtimeIds)));
    },
    [],
  );

  const loadFreshIdentity = React.useCallback(async () => {
    setIsPending(true);
    setError(null);
    try {
      if (identityLost) {
        await recoverLostIdentity();
        return;
      }
      const identity = await getIdentity();
      queryClient.setQueryData(["identity"], identity);
      setIdentity(identity);
    } catch (cause) {
      setError(
        cause instanceof Error ? cause.message : "Failed to load identity",
      );
    } finally {
      setIsPending(false);
    }
  }, [identityLost, queryClient]);

  return (
    <div
      className={`buzz-onboarding-neutral-theme buzz-startup-shell flex max-h-dvh items-start justify-center overflow-x-hidden overflow-y-auto px-4 text-foreground ${
        page === "identity"
          ? "buzz-onboarding-welcome py-8"
          : "pb-28 pt-[106px]"
      }`}
      data-testid="machine-onboarding-gate"
    >
      <StartupWindowDragRegion />
      {page !== "identity" ? (
        <OnboardingChrome current={page === "config" ? 3 : 2} />
      ) : null}
      <OnboardingFooterProvider>
        <div
          className={`relative flex w-full max-w-[1040px] flex-col items-center text-center ${
            page === "identity" ? "my-auto" : "buzz-onboarding-step-frame"
          }`}
        >
          {page === "identity" ? (
            <OnboardingSlideTransition
              className="flex w-full max-w-[720px] flex-col items-center text-center"
              direction="forward"
              effect="mask-reveal-up"
              transitionKey="machine-identity"
            >
              <TicTacToeMark
                ariaLabel="tic-tac-toe"
                className="h-28 w-28 shadow-2xl"
              />
              <h1 className="mt-7 text-5xl font-semibold tracking-[-0.055em] text-foreground sm:text-6xl">
                tic-tac-toe
              </h1>
              <p className="mt-2 font-mono text-sm uppercase tracking-[0.24em] text-muted-foreground">
                an x0x app
              </p>
              <p className="mt-2 max-w-[560px] text-center text-2xl font-normal leading-none text-foreground">
                The winning move is to live together in peace.
              </p>
              {identity ? (
                <div
                  className="mt-8 w-full max-w-xl rounded-2xl border border-border/70 bg-background/65 p-5 text-left shadow-sm backdrop-blur"
                  data-testid="onboarding-x0x-identity"
                >
                  <p className="text-xs font-medium uppercase tracking-[0.16em] text-muted-foreground">
                    Your x0x identity
                  </p>
                  <p
                    className="mt-2 text-xl font-semibold tracking-tight text-foreground"
                    data-testid="onboarding-identity-words"
                  >
                    {identity.identityWords.join(" ")}
                  </p>
                  <p
                    className="mt-3 break-all font-mono text-xs leading-5 text-muted-foreground"
                    data-testid="onboarding-agent-id"
                  >
                    {identity.agentId}
                  </p>
                  <p className="mt-3 text-sm leading-5 text-muted-foreground">
                    Share a signed agent card or the full Agent ID when adding a
                    contact. The four words are a memorable display prefix, not
                    a globally unique address.
                  </p>
                </div>
              ) : null}
              {error ? (
                <p className="mt-4 text-sm text-destructive">{error}</p>
              ) : null}
              <div className="mt-10 flex flex-col items-center gap-3">
                <Button
                  className={ONBOARDING_LANDING_CTA_CLASS}
                  disabled={isPending}
                  onClick={() => {
                    if (identity) {
                      setPage("setup");
                      return;
                    }
                    void loadFreshIdentity();
                  }}
                  type="button"
                >
                  {isPending
                    ? "Setting up…"
                    : identity
                      ? "Continue"
                      : "Get started"}
                </Button>
              </div>
            </OnboardingSlideTransition>
          ) : page === "setup" ? (
            <SetupStep
              actions={{
                back: () => setPage("identity"),
                next: (runtimeIds) => {
                  const ids = Array.from(runtimeIds);
                  setReadyRuntimeIds(ids);
                  // Harness install can fail (Windows/PATH/network). Don't soft-lock
                  // onboarding — users can finish setup later in Settings → Agents.
                  if (ids.length === 0) {
                    complete();
                    return;
                  }
                  setPage("config");
                },
              }}
              direction="forward"
              onReadyRuntimeIdsChange={handleReadyRuntimeIdsChange}
            />
          ) : (
            <DefaultConfigStep
              actions={{
                back: () => setPage("setup"),
                complete: () => complete(),
              }}
              direction="forward"
              readyRuntimeIds={readyRuntimeIds}
            />
          )}
        </div>
      </OnboardingFooterProvider>
    </div>
  );
}

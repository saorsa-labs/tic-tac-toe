import { relaunch } from "@tauri-apps/plugin-process";

import { useSystemColorScheme } from "@/shared/theme/useSystemColorScheme";
import { Button } from "@/shared/ui/button";
import { StartupWindowDragRegion } from "@/shared/ui/StartupWindowDragRegion";

export function KeyringLockedScreen() {
  const systemColorScheme = useSystemColorScheme();

  return (
    <div
      className="buzz-onboarding-neutral-theme buzz-startup-shell flex items-center justify-center bg-background px-4 py-8 text-foreground"
      data-system-color-scheme={systemColorScheme}
      data-testid="keyring-locked"
    >
      <StartupWindowDragRegion />
      <div className="relative flex w-full max-w-[500px] flex-col items-center text-center">
        <h1 className="text-3xl font-semibold tracking-tight">
          Compatibility service unavailable
        </h1>
        <p className="mt-3 text-sm leading-6 text-muted-foreground">
          Buzz couldn&apos;t reach the compatibility service that backs your
          identity. Unlock your desktop session or restart the service, then
          relaunch Buzz.
        </p>
        <Button
          className="mt-8 h-10 w-full max-w-[300px]"
          data-testid="relaunch-app"
          onClick={() => {
            void relaunch();
          }}
          type="button"
        >
          Relaunch Buzz
        </Button>
      </div>
    </div>
  );
}

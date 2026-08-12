import { RecoveryScreen } from "./RecoveryScreen";

export function RelaunchRequiredScreen() {
  return (
    <RecoveryScreen
      testId="relaunch-required"
      title="Restart tic-tac-toe to finish recovery"
      body="Your identity was updated. tic-tac-toe needs to restart so syncing and agents run under it."
    />
  );
}

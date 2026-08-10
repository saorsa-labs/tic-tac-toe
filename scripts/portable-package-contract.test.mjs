import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { describe, it } from "node:test";

const [tauriConfigSource, distSource, launcherSource, entitlementsSource] =
  await Promise.all([
    readFile("desktop/src-tauri/tauri.conf.json", "utf8"),
    readFile("scripts/dist.sh", "utf8"),
    readFile("scripts/run-tic-tac-toe.sh", "utf8"),
    readFile("desktop/scripts/verify-macos-entitlements.sh", "utf8"),
  ]);

const tauriConfig = JSON.parse(tauriConfigSource);
const expectedBundleName = `${tauriConfig.productName}.app`;
const legacyIdentityEnvironment = /\b(?:BUZZ_PRIVATE_KEY|NOSTR_PRIVATE_KEY|BUZZ_RELAY_URL)\b/;

describe("portable macOS package contract", () => {
  it("uses the exact Tauri productName bundle throughout distribution", () => {
    assert.equal(expectedBundleName, "tic-tac-toe.app");
    assert.match(distSource, /APP_BUNDLE_NAME="tic-tac-toe\.app"/);
    assert.match(
      distSource,
      /APP_BUNDLE="\$DESKTOP_DIR\/src-tauri\/target\/release\/bundle\/macos\/\$APP_BUNDLE_NAME"/,
    );
    assert.match(distSource, /"\$PKG_DIR\/\$APP_BUNDLE_NAME"/);
    assert.match(
      distSource,
      /tar -czf "\$TARBALL_TMP" -C "\$PKG_DIR" "\$APP_BUNDLE_NAME"/,
    );
    assert.match(launcherSource, /APP_BUNDLE_NAME="tic-tac-toe\.app"/);
    assert.match(
      launcherSource,
      /"\$DIR\/\$APP_BUNDLE_NAME\/Contents\/MacOS\/buzz-desktop"/,
    );
    assert.match(
      launcherSource,
      /"\$DIR\/\$APP_BUNDLE_NAME\/Contents\/MacOS\/x0xd"/,
    );
    assert.match(entitlementsSource, /path-to-tic-tac-toe\.app/);
    assert.doesNotMatch(
      `${distSource}\n${launcherSource}\n${entitlementsSource}`,
      /\bBuzz\.app\b/,
    );
  });

  it("never injects retired relay identity into the portable desktop", () => {
    assert.doesNotMatch(launcherSource, legacyIdentityEnvironment);
  });
});

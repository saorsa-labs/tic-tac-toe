# Stage 0 cut list — features disabled in Stage 1

Per `buzz-fork-plan.md` (§Stage 0/§5 of the seam review): BuilderLab hosted
onboarding, mobile pairing, and git hosting are cut for v1. Stage 0 imports
the code untouched; **Stage 1 disables these entry points**. Every path below
is verified present in the imported tree @ upstream `7e34bee6`.

## 1. BuilderLab hosted-community onboarding (hardcoded Block service)

Rust:
- `desktop/src-tauri/src/builderlab.rs` — `BUILDERLAB_ORIGIN = "https://app.builderlab.xyz"` (line 22); all `/v1/buzz/*` calls
- `desktop/src-tauri/src/nostr_bind.rs` — email→Nostr identity bind (BuilderLab-bound)
- `desktop/src-tauri/src/deep_link.rs` — the `buzz://nostr-bind` route only (other `buzz://` routes work against any relay/bridge and stay)

TS:
- `desktop/src/features/communities/hostedCommunityApi.ts` (`communities.buzz.xyz`)
- `desktop/src/features/communities/ui/HostedCommunityOnboarding.tsx`
- `desktop/src/features/communities/ui/WelcomeSetup.tsx` (hosted path only — local-relay onboarding via `AddCommunityDialog` is the supported path and stays)
- `desktop/src/features/settings/ui/HostedCommunitiesSettingsCard.tsx`

## 2. Mobile device pairing

- `desktop/src-tauri/src/commands/pairing.rs` (NIP-11 `pairing_relay_url` discovery + `/pair`)
- `desktop/src-tauri/src/commands/qr_download.rs`
- UI entry points referencing pairing/QR (locate via `grep -ri "pairing" desktop/src/features/settings` at Stage 1)

## 3. Git hosting (Projects)

- `desktop/src-tauri/src/commands/project_git.rs`, `project_git_branches.rs`, `project_git_diff.rs`, `project_git_exec.rs`, `project_git_push.rs`, `project_git_workflow.rs`
- Routes: `desktop/src/app/routes/projects.tsx`, `projects.$projectId.tsx` (hide; event-based PR/issue flows that ride the relay WS may return later — Stage 1 hides the routes wholesale)

## 4. Huddle voice (separate decision record)

Cut per `voice-over-x0x.md` — voice returns P2P via saorsa-webrtc:
- `desktop/src-tauri/src/huddle/` (19 files; also downloads ~189 MB TTS models from external URLs)
- UI huddle entry points (Stage 1: hide call buttons until ttt V2 voice)

## Disable mechanism (Stage 1)

Prefer a single `TTT_FEATURES` gate (build-time env or config) consulted at
route registration / command registration, over deleting imported code —
keeps the tree cherry-pickable against upstream. Per-file `#[cfg(feature)]`
in Rust and route-level guards in TanStack Router on the TS side.

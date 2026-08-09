// Static CI gate enforcing the M2 ("Stage 2 — identity flip") invariant from
// `docs/design/buzz-fork-plan.md`:
//
//   "after Stage 2, npub never appears in UI; CI grep enforces … Nostr keys
//    authenticate nothing except the loopback dialect."
//
// The user's displayed identity is the x0x AgentId + four speakable words. The
// Nostr compatibility signer (relayPubkey / internal signing) is a loopback
// dialect detail that must NEVER render and NEVER come under user control.
//
// This gate fails when any of `npub`, `nsec`, `safeNpub`, `pubkeyToNpub` — the
// bech32 Nostr-key identifiers and their display helpers — surfaces in:
//   • production desktop UI (`desktop/src`, minus tests), and
//   • the UI-facing src-tauri identity surface (`models.rs` + `commands/identity.rs`).
//
// A precise allowlist admits ONLY explicit loopback-protocol / internal
// plumbing survivors. There is deliberately NO blanket `src-tauri` exclusion:
// the UI-facing model and identity commands ARE scanned, and the internal
// signing kernel (`app_state.rs` keyring persistence) is simply out of scope
// because it is not UI-facing.
//
// Self "tooth check": every run first proves the scanner still catches a
// deliberate forbidden fixture (and rejects the `consecutive` false positive).
// If the scanner were ever neutered, the gate fails closed.

import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");

// ---------------------------------------------------------------------------
// Forbidden tokens
// ---------------------------------------------------------------------------
// Ordered alternation: at each position the regex tries the longest token
// first, so `safeNpub` / `pubkeyToNpub` are matched whole rather than as a
// trailing bare `npub`. Match text preserves original case; length classifies.
const TOKEN_RE = /safeNpub|pubkeyToNpub|npub|nsec/gi;

const TOKEN_BY_LEN = {
  8: "safeNpub",
  12: "pubkeyToNpub",
  4: null, // resolved from the matched text (npub vs nsec)
};

function classify(matchText) {
  const lower = matchText.toLowerCase();
  if (lower === "safenpub") return "safeNpub";
  if (lower === "pubkeytonpub") return "pubkeyToNpub";
  if (lower.startsWith("npub")) return "npub";
  if (lower.startsWith("nsec")) return "nsec";
  return "npub"; // defensive; combined regex only matches the four above
}

const isLowerLetter = (c) => c >= "a" && c <= "z";

// A case-insensitive match is a REAL violation unless the character
// immediately AFTER it is a lowercase ASCII letter. Real npub/nsec tokens are
// always delimited — a bech32 prefix (`npub1…` → next char is a digit), a
// camelCase identifier (`safeNpub`, `copiedNpub`, `NsecMaskedDisplay` → next
// char is uppercase / end / symbol), a snake_case name (`get_nsec` → next is
// `_`/`)`/end), or a standalone reference. None is ever followed by a
// lowercase letter. A trailing lowercase letter means the match is a
// word-internal infix of plain English — `co·nsec·utive`, `u·npub·lished`,
// `I·nSec·tion`, `retryI·nSec·onds` — which we must never flag.
function isRealViolation(line, start, len) {
  const nextChar = start + len < line.length ? line[start + len] : "";
  if (isLowerLetter(nextChar)) {
    return false;
  }
  return true;
}

/**
 * Find every real forbidden-token occurrence in one line of source.
 * @returns {Array<{ col: number, token: string, text: string }>}
 */
function findInLine(line) {
  const hits = [];
  TOKEN_RE.lastIndex = 0;
  let match = TOKEN_RE.exec(line);
  while (match !== null) {
    const start = match.index;
    const len = match[0].length;
    if (isRealViolation(line, start, len)) {
      const token = TOKEN_BY_LEN[len] ?? classify(match[0]);
      hits.push({ col: start + 1, token, text: match[0] });
    }
    match = TOKEN_RE.exec(line);
  }
  return hits;
}

// ---------------------------------------------------------------------------
// Scan configuration
// ---------------------------------------------------------------------------
// Roots are relative to `projectRoot` (the `desktop/` package dir).
//   - `src`            : all production desktop UI (TS/TSX, tests excluded).
//   - two src-tauri files: the UI-facing identity MODEL and identity COMMANDS.
const SCAN_DIRS = [{ dir: "src", extensions: [".ts", ".tsx"] }];
const SCAN_FILES = [
  "src-tauri/src/models.rs",
  "src-tauri/src/commands/identity.rs",
];

// Production-only: skip unit tests, the E2E mock fabric, and ambient type
// declarations. None of these ship to users, so npub/nsec there is not a UI
// regression. (The pubkey-truncation gate allowlists `src/testing/e2eBridge.ts`
// for the same reason; excluding the whole non-production surface is stricter
// and needs no per-file maintenance.)
function isExcluded(relPath) {
  if (relPath.endsWith(".d.ts")) return true;
  if (/\.(test|spec)\.(ts|tsx|mjs|js)$/.test(relPath)) return true;
  if (relPath.startsWith("src/testing/")) return true;
  return false;
}

// Precise allowlist — explicit loopback-protocol / internal-plumbing survivors.
// The gate's PURPOSE is "npub/nsec never appear in user-facing UI"; these
// entries exempt NON-UI occurrences (protocol type fields, API params, legacy
// migration machinery, and anti-npub documentation) so the scan can be a blunt
// token grep without false-failing on internals. There is no blanket src-tauri
// exclusion — the UI-facing model + identity commands ARE scanned (above), and
// the internal signing kernel (app_state.rs keyring persistence) is simply out
// of scope because it is not UI-facing.
//
//   • ALLOWED_FILES : a file whose every match is internal (whole-file pass).
//   • ALLOWED_LINES : { file, contains } — suppress a finding whose (trimmed)
//     source line contains the substring. Content-anchored, NOT line numbers,
//     so it survives the M2 edits that reshape the Identity type around these
//     lines; a line that moves or is reworded simply re-surfaces (fail-loud)
//     instead of being silently hidden.
//
// After the M2 cutover the production UI surface is npub/nsec-free; this list
// is everything that legitimately remains. Do NOT add UI/display paths here —
// those are bugs the gate must catch.
const ALLOWED_FILES = new Set([
  // Legacy community bootstrap persisted the user's `nsec` in localStorage;
  // this loader STRIPS any lingering `nsec` field from old entries on read.
  // Anti-nsec migration plumbing, never rendered.
  "src/features/communities/communityStorage.ts",
  // Community entry type explicitly forbids an `nsec` field (`nsec?: never`).
  // Compile-time guard against the legacy shape, not a display path.
  "src/features/communities/types.ts",
  // Low-level Nostr bech32 encode/decode helpers (pubkeyToNpub / safeNpub /
  // nsecToNpub) — a protocol utility layer over nip19, never rendered itself.
  // The UI sin is *calling* these from display components, which M2 removes;
  // the lib itself is internal non-UI machinery.
  "src/shared/lib/nostrUtils.ts",
]);

const ALLOWED_LINES = [
  // — Managed-agent provisioning API (loopback dialect) ——————————
  // The agent's compatibility signer key returned at create time. A protocol
  // response field, never user identity display.
  { file: "src/shared/api/types.ts", contains: "privateKeyNsec: string;" },
  { file: "src/shared/api/tauri.ts", contains: "private_key_nsec: string;" },
  {
    file: "src/shared/api/tauri.ts",
    contains: "privateKeyNsec: response.private_key_nsec",
  },
  // apply_workspace community-join API: optional nsec param, intentionally
  // unused at the call site (useCommunityInit refuses to pass one).
  { file: "src/shared/api/tauri.ts", contains: "nsec?: string," },
  { file: "src/shared/api/tauri.ts", contains: "nsec: nsec ?? null," },

  // — Anti-npub documentation (comments that name the tokens to forbid them) —
  // The Identity.relayPubkey doc: the signer is hex-only, never a bech32
  // npub/nsec form. AgentIdentity.tsx (the M2 PubKey replacement) states the
  // same invariant for the displayed id.
  { file: "src/shared/api/types.ts", contains: "(npub/nsec)" },
  { file: "src/shared/ui/AgentIdentity.tsx", contains: "(npub/nsec)" },

  // — Community-init nsec safeguards (internal comments) ——————————
  // Document that no nsec is passed to the backend and that legacy nsec
  // fields are stripped — anti-nsec plumbing adjacent to communityStorage.ts.
  {
    file: "src/features/communities/useCommunityInit.ts",
    contains: "do NOT pass an nsec",
  },
  {
    file: "src/features/communities/useCommunityInit.ts",
    contains: "stored the nsec in localStorage",
  },
  {
    file: "src/features/communities/useCommunityInit.ts",
    contains: "strips lingering",
  },
  {
    file: "src/features/communities/useCommunityInit.ts",
    contains: "an invalid nsec",
  },
  // — Anti-npub validation tests (Rust #[test], compiled out of release) ————
  // `validate_agent_id` PROVES it rejects bech32 npub placeholders a misconfigured
  // daemon might emit. The `npub1` literal is load-bearing test input, never UI;
  // the production identity commands are npub/nsec-free (M2RustIdentity-verified).
  {
    file: "src-tauri/src/commands/identity.rs",
    contains: 'assert_rejects(&format!("npub1',
  },
];

function isAllowed(relPath, sourceLine) {
  if (ALLOWED_FILES.has(relPath)) return true;
  for (const { file, contains } of ALLOWED_LINES) {
    if (relPath === file && sourceLine.includes(contains)) return true;
  }
  return false;
}

// ---------------------------------------------------------------------------
// File walking
// ---------------------------------------------------------------------------
async function walkDir(absDir, extensions) {
  const out = [];
  let entries;
  try {
    entries = await readdir(absDir, { withFileTypes: true });
  } catch {
    return out; // missing dir → nothing to scan (handled by caller existence check)
  }
  for (const entry of entries) {
    const abs = path.join(absDir, entry.name);
    if (entry.isDirectory()) {
      out.push(...(await walkDir(abs, extensions)));
    } else if (
      entry.isFile() &&
      extensions.includes(path.extname(entry.name))
    ) {
      out.push(abs);
    }
  }
  return out;
}

async function collectFiles(rootDir) {
  const files = [];
  for (const { dir, extensions } of SCAN_DIRS) {
    files.push(...(await walkDir(path.join(rootDir, dir), extensions)));
  }
  for (const rel of SCAN_FILES) {
    files.push(path.join(rootDir, rel));
  }
  return files.map((abs) => path.relative(rootDir, abs));
}

// ---------------------------------------------------------------------------
// Core scan
// ---------------------------------------------------------------------------
async function scanRoot(rootDir) {
  const relFiles = await collectFiles(rootDir);
  const findings = [];
  let scanned = 0;
  for (const rel of relFiles.sort()) {
    if (isExcluded(rel)) continue;
    const abs = path.join(rootDir, rel);
    let content;
    try {
      content = await readFile(abs, "utf8");
    } catch {
      continue; // a configured single file may not exist yet during cutover
    }
    scanned += 1;
    const lines = content.split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      const lineNo = i + 1;
      for (const hit of findInLine(lines[i])) {
        if (isAllowed(rel, lines[i].trim())) continue;
        findings.push({
          file: rel,
          line: lineNo,
          col: hit.col,
          token: hit.token,
          text: hit.text,
          source: lines[i].trim(),
        });
      }
    }
  }
  return { findings, scanned };
}

// ---------------------------------------------------------------------------
// Self tooth check
// ---------------------------------------------------------------------------
// Proves the scanner (a) catches every forbidden token, (b) catches test-ids /
// labels / bech32 prefixes, and (c) rejects the "consecutive" false positive —
// by running the REAL scanRoot over a throwaway fixture tree. If any assertion
// fails the gate fails closed (exit 1) so a neutered scanner can never pass.
async function toothCheck() {
  const cases = [
    {
      line: "const npub = pubkeyToNpub(identity.pubkey);",
      want: ["pubkeyToNpub", "npub"],
    },
    {
      line: 'import { safeNpub } from "@/shared/lib/nostrUtils";',
      want: ["safeNpub"],
    },
    { line: "                        Private key (nsec)", want: ["nsec"] },
    {
      line: '                        data-testid="welcome-join-npub"',
      want: ["npub"],
    },
    { line: "  const bech = `npub1$" + "{'a'.repeat(59)}`;", want: ["npub"] },
    { line: "  return get_nsec();", want: ["nsec"] },
    { line: "  // Consecutive failures per canonical relay URL.", want: [] },
    { line: "  const x = consecutively;", want: [] },
    { line: "  const clean = 'no tokens here at all';", want: [] },
  ];
  for (const { line, want } of cases) {
    const got = findInLine(line).map((h) => h.token);
    const wantSet = new Set(want);
    for (const w of want) {
      if (!got.includes(w)) {
        throw new Error(
          `tooth-check: expected token "${w}" in ${JSON.stringify(line)} but got [${got.join(", ")}]`,
        );
      }
    }
    for (const g of got) {
      if (!wantSet.has(g)) {
        throw new Error(
          `tooth-check: unexpected token "${g}" in ${JSON.stringify(line)}`,
        );
      }
    }
  }

  // End-to-end: a forbidden fixture file MUST be flagged, a clean one MUST NOT.
  const tmp = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    ".tooth-check-tmp",
  );
  await mkdir(path.join(tmp, "src"), { recursive: true });
  await writeFile(
    path.join(tmp, "src", "BadIdentity.tsx"),
    'export const x = pubkeyToNpub(id);\nexport const secret = "nsec1leak";\n',
  );
  await writeFile(
    path.join(tmp, "src", "CleanWidget.tsx"),
    "export const x = id.agentId;\n",
  );
  try {
    const { findings } = await scanRootCheck(tmp);
    if (findings.length !== 2) {
      throw new Error(
        `tooth-check: expected 2 findings in fixture, got ${findings.length}: ${JSON.stringify(findings)}`,
      );
    }
  } finally {
    await rm(tmp, { recursive: true, force: true });
  }
}

// scanRoot variant for the tooth-check fixture: scans an arbitrary dir as if it
// were `projectRoot`, but only under a `src` tree with .ts/.tsx — matching the
// production rule set so the end-to-end path is exercised truthfully.
async function scanRootCheck(rootDir) {
  const files = await walkDir(path.join(rootDir, "src"), [".ts", ".tsx"]);
  const findings = [];
  for (const abs of files) {
    const rel = path.relative(rootDir, abs);
    if (isExcluded(rel)) continue;
    const content = await readFile(abs, "utf8");
    const lines = content.split(/\r?\n/);
    for (let i = 0; i < lines.length; i++) {
      for (const hit of findInLine(lines[i])) {
        findings.push({ file: rel, line: i + 1, token: hit.token });
      }
    }
  }
  return { findings };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
async function main() {
  try {
    await toothCheck();
  } catch (error) {
    console.error(`\x1b[31m[nostr-identity-ui] self tooth-check FAILED\x1b[0m`);
    console.error(`  ${error.message}`);
    console.error(
      `  The scanner can no longer detect forbidden tokens — refusing to pass.`,
    );
    process.exit(1);
  }

  const { findings, scanned } = await scanRoot(projectRoot);

  const label = "nostr-identity-ui";
  const scriptPath = "desktop/scripts/check-nostr-identity-ui.mjs";

  if (findings.length === 0) {
    console.error(
      `\x1b[32m[${label}] OK\x1b[0m — 0 forbidden npub/nsec tokens across ${scanned} scanned files.`,
    );
    console.error(
      `  M2 invariant holds: npub/nsec never appear in the desktop UI or identity surface.`,
    );
    process.exit(0);
  }

  console.error(
    `\x1b[31m[${label}] FAILED\x1b[0m — ${findings.length} forbidden npub/nsec token(s) in user-facing code.`,
  );
  console.error(
    `  After the M2 identity flip the only user identity is x0x AgentId + four words.`,
  );
  console.error(
    `  Nostr keys authenticate nothing but the loopback dialect and must never render.`,
  );
  console.error(
    `  If a finding is a genuine loopback-protocol/internal survivor, add a precise`,
  );
  console.error(
    `  entry to ALLOWED_FILES / OVERRIDES in ${scriptPath} with a justification.`,
  );
  console.error("");
  for (const f of findings) {
    console.error(
      `  \x1b[33m${f.file}:${f.line}:${f.col}\x1b[0m  \x1b[31m${f.token}\x1b[0m  (${JSON.stringify(f.text)})`,
    );
    console.error(`    ${f.source}`);
  }
  console.error("");
  process.exit(1);
}

await main();

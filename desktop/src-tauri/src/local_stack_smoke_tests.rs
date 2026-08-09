//! Native real-process acceptance tests for the local x0xd stack.
//!
//! These are `#[ignore]`d: they spawn the REAL `x0xd` from `TTT_X0XD_BINARY`
//! (or the sibling debug build) and need exclusive access to a loopback port +
//! process group. Run them in isolation:
//!
//! ```text
//! cd desktop/src-tauri
//! TTT_X0XD_BINARY=../../../x0x/target/debug/x0xd \
//!   cargo test --lib local_stack::tests::smoke -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Every test owns and reaps its child (via [`NativeDaemonFixture`], whose
//! `Drop` is fail-closed), uses an isolated temp data dir + HOME, binds only to
//! loopback, and disables bootstrap peers so the daemon never joins the public
//! network. No relay, Nostr, x0x-nostr-bridge, mock IPC, or secure-group
//! cryptography is on the path — only x0xd's authenticated loopback REST API.
//!
//! Scope note: the two-daemon public/DM history restart+search acceptance is
//! intentionally NOT here. x0xd records durable history (`GET /history/search`)
//! only for messages delivered over the gossip mesh and verified via the
//! ML-DSA-65 secure envelope path — which needs two connected daemons + identity
//! trust + the secure-group machinery this harness forbids, plus it would
//! duplicate protocol logic. The reusable [`NativeDaemonFixture`] is the
//! extension point: Main can spin up a second fixture, connect the two over
//! loopback, exchange a DM, restart one, and search — with no bridge fields to
//! work around.

#![allow(dead_code)]
use super::native_fixture::*;
use super::*;

/// (1) Spawn → authenticated `/health` readiness → (2) shutdown/reap ownership.
///
/// Defends: the real `x0xd` writes `api.port`/`api-token` in the exact format
/// the production readers parse; the production [`LoopbackHttpDaemonProbe`]
/// `/health` gate succeeds against a real authenticated daemon while it runs;
/// and the production [`StdSidecarSpawner`] reap path actually kills it, after
/// which `/health` fails and the API port is freed.
#[cfg(unix)]
#[test]
#[ignore = "native real-process: needs TTT_X0XD_BINARY (or ../x0x/target/debug/x0xd); run with --ignored --test-threads=1"]
fn native_spawn_health_then_reap() {
    let mut fx = NativeDaemonFixture::start();
    let api_base = fx.api_base();
    let token = fx.api_token().to_string();
    let port = fx.api_port();

    // Readiness already proven by `start`, but re-assert the contract directly:
    // the production probe must accept the real authenticated /health.
    assert!(
        LoopbackHttpDaemonProbe.health(&api_base, &token).is_ok(),
        "daemon /health must be healthy while owned",
    );

    // Reap via the PRODUCTION OwnedChild path (SIGTERM→SIGKILL on the pgrp).
    let mut child = fx.take_child().expect("fixture owns the daemon child");
    child.shutdown();

    // Poll death with a deadline — never a fixed sleep.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let healthy = LoopbackHttpDaemonProbe.health(&api_base, &token).is_ok();
        if !healthy {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "daemon still healthy 10s after reap; see {}",
            fx.data_dir().join("x0xd.log").display(),
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    // Defence in depth: the API TCP port is no longer bound by any process.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !listen_pids_on(port).is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        listen_pids_on(port).is_empty(),
        "api port {port} still bound after reap",
    );
}

/// (1, attach leg) A running daemon is ATTACHED (not respawned) by the
/// production supervisor when its artifacts + health check already resolve.
///
/// Defends: [`LocalStackSupervisor::bring_up`] takes the attach path
/// (`handle.daemon` is `None`, no spawn) when a healthy daemon already owns the
/// data dir, and the returned `api_base` matches the running daemon. This is
/// the M1a spawn-or-attach contract against a real process.
#[cfg(unix)]
#[test]
#[ignore = "native real-process: needs TTT_X0XD_BINARY (or ../x0x/target/debug/x0xd); run with --ignored --test-threads=1"]
fn native_attach_reuses_running_daemon_without_spawn() {
    let fx = NativeDaemonFixture::start();
    let expected_api_base = fx.api_base();

    // Production supervisor pointed at the fixture's data dir. The spawner is
    // real but must NOT be invoked on the attach path.
    let cfg = StackConfig {
        data_dir: fx.data_dir().to_path_buf(),
        x0xd_binary: resolve_x0xd_binary(),
        daemon_timeout: Duration::from_secs(5),
    };
    let sup = LocalStackSupervisor::new(
        cfg,
        LoopbackHttpDaemonProbe,
        StdSidecarSpawner,
        BlockingTimeSource,
    );

    let handle = sup
        .bring_up()
        .expect("attach to the running daemon must succeed");
    assert!(
        handle.daemon.is_none(),
        "attach must NOT take ownership of (or respawn) the running daemon",
    );
    assert_eq!(
        handle.api_base, expected_api_base,
        "attached api_base must match the running daemon",
    );

    // Shutting down an attached handle must NOT kill the daemon we don't own.
    let mut handle = handle;
    handle.shutdown();
    assert!(
        LoopbackHttpDaemonProbe
            .health(&expected_api_base, fx.api_token())
            .is_ok(),
        "attached daemon must survive handle shutdown (we don't own it)",
    );
    // fx Drop reaps the real daemon.
}

/// (3) Unix listener isolation: every TCP LISTEN socket owned by the spawned
/// x0xd is bound to loopback, and the authenticated API port is among them.
/// After shutdown there are no survivors. Skipped (NOT passed) without `lsof`.
///
/// Defends: the app-owned daemon never exposes a non-loopback (wildcard /
/// `0.0.0.0` / `::`) TCP listener — the security-critical property. The QUIC
/// P2P transport is UDP and the outbound update/telemetry dial is not a LISTEN
/// socket, so neither can masquerade as a public listening surface here.
#[cfg(unix)]
#[test]
#[ignore = "native real-process isolation: needs TTT_X0XD_BINARY (or ../x0x/target/debug/x0xd) + lsof; run with --ignored --nocapture --test-threads=1"]
fn native_tcp_listeners_are_loopback_only() {
    if !lsof_available() {
        eprintln!("skip native_tcp_listeners_are_loopback_only: lsof not available");
        return;
    }

    let mut fx = NativeDaemonFixture::start();
    let port = fx.api_port();

    // Resolve the owned PID from the known API port (scoped lookup, not a
    // host-wide scan, so unrelated listeners never enter the assertion).
    let owned_pids = unique_sorted_u32(listen_pids_on(port));
    assert!(
        !owned_pids.is_empty(),
        "no owned PID resolved on the api port {port}",
    );
    eprintln!("owned listener PIDs: {owned_pids:?}");

    // Enumerate EVERY TCP LISTEN socket of each owned PID (catches extra binds).
    let mut observed: Vec<(u32, String, u16)> = Vec::new(); // (pid, host, port)
    for &pid in &owned_pids {
        for (host, p) in pid_listen_sockets(pid) {
            observed.push((pid, host, p));
        }
    }
    assert!(
        !observed.is_empty(),
        "owned PIDs expose no TCP LISTEN sockets"
    );
    for (pid, host, p) in &observed {
        eprintln!("LISTEN pid={pid} {host}:{p}");
    }

    // HARD: every owned bind is loopback. Wildcard / 0.0.0.0 / :: fail here.
    for (pid, host, p) in &observed {
        assert!(
            is_loopback_host(host),
            "non-loopback TCP bind by owned pid {pid}: {host}:{p}",
        );
    }

    // The authenticated API port must be among the loopback listeners.
    assert!(
        observed.iter().any(|(_, _, p)| *p == port),
        "api port {port} not found among loopback listeners",
    );

    // Shutdown, then poll that every owned PID has exited (no fixed sleep).
    fx.shutdown();
    let deadline = Instant::now() + Duration::from_secs(10);
    let survivors = || -> Vec<u32> {
        owned_pids
            .iter()
            .copied()
            .filter(|p| process_running(*p))
            .collect()
    };
    let mut remaining = survivors();
    while Instant::now() < deadline && !remaining.is_empty() {
        std::thread::sleep(Duration::from_millis(200));
        remaining = survivors();
    }
    assert!(
        remaining.is_empty(),
        "owned PIDs did not exit after shutdown: {remaining:?}",
    );
    assert!(
        listen_pids_on(port).is_empty(),
        "api port {port} still bound after shutdown",
    );
}

// ─── (4) Two-node public-group durable-history acceptance ───────────────
//
// The v1 zero-server proof point: two FRESH isolated x0xd nodes discover,
// pair, exchange a SignedPublic root message over the gossip mesh, the sender
// is terminated and respawned on the same data dir, and its DURABLE history
// (SQLite, ADR-0023) still holds the exact message — with no relay, Nostr,
// x0x-nostr-bridge, mock IPC, or secure-group cryptography on the path. Only
// x0xd's authenticated loopback REST API is exercised; the message is carried
// peer-to-peer over the loopback QUIC mesh (bob bootstraps explicitly to
// alice). This test reuses the production spawn/reap/probe plumbing via
// [`NativeDaemonFixture`] and never duplicates daemon protocol logic.

/// Authenticated loopback REST client (Bearer token per daemon).
fn authed_client(token: &str) -> reqwest::blocking::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .expect("auth header encodes"),
    );
    reqwest::blocking::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(20))
        .build()
        .expect("authed blocking client")
}

fn rest_get(client: &reqwest::blocking::Client, url: &str) -> serde_json::Value {
    client
        .get(url)
        .send()
        .unwrap_or_else(|e| panic!("GET {url} failed: {e}"))
        .json::<serde_json::Value>()
        .unwrap_or_else(|e| panic!("GET {url} body not json: {e}"))
}

fn rest_post(
    client: &reqwest::blocking::Client,
    url: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    client
        .post(url)
        .json(&body)
        .send()
        .unwrap_or_else(|e| panic!("POST {url} failed: {e}"))
        .json::<serde_json::Value>()
        .unwrap_or_else(|e| panic!("POST {url} body not json: {e}"))
}

/// Deadline-polled boolean condition — never a fixed sleep.
fn wait_until<F: FnMut() -> bool>(timeout: Duration, mut check: F) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if check() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn agent_id(client: &reqwest::blocking::Client, api: &str) -> String {
    rest_get(client, &format!("{api}/agent"))["agent_id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn agent_card_link(client: &reqwest::blocking::Client, api: &str) -> String {
    rest_get(client, &format!("{api}/agent/card"))["link"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn peer_count(client: &reqwest::blocking::Client, api: &str) -> usize {
    let peers = rest_get(client, &format!("{api}/peers"));
    peers
        .as_array()
        .or_else(|| peers["peers"].as_array())
        .map_or(0, |entries| entries.len())
}

/// `(direct, relayed)` connection counts from a daemon's
/// `/diagnostics/connectivity`. A direct loopback link ⇒ `direct >= 1,
/// relayed == 0`: the alice↔bob transport is peer-to-peer, NOT relayed.
/// Scoped to the queried daemon's own transport (never a host-wide process
/// scan), so unrelated daemons or stray bridge binaries cannot trip it.
fn connection_split(client: &reqwest::blocking::Client, api: &str) -> (u64, u64) {
    let c = rest_get(client, &format!("{api}/diagnostics/connectivity"));
    let direct = c["connections"]["direct"].as_u64().unwrap_or(0);
    let relayed = c["connections"]["relayed"].as_u64().unwrap_or(0);
    (direct, relayed)
}

/// Decode a base64 history `payload` to UTF-8 (returns `None` on any failure).
fn decode_b64_payload(b64: &str) -> Option<String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    String::from_utf8(bytes).ok()
}

/// Owns both fixtures and reaps them in deterministic reverse order — even on
/// assertion-failure unwind. Each fixture's own `Drop` is the backstop.
struct PairGuard {
    alice: NativeDaemonFixture,
    bob: NativeDaemonFixture,
}

impl Drop for PairGuard {
    fn drop(&mut self) {
        self.bob.shutdown();
        self.alice.shutdown();
    }
}

/// (4) Two-node SignedPublic group: pair → send → live-observe → sender
/// restart → durable history still holds the exact message. No relay/bridge.
///
/// Defends, per leg: (topology) two loopback-isolated daemons form a gossip
/// mesh via explicit bootstrap, no public network; (pairing) the supported
/// `/agent/card` + `/agents/connect` flow establishes trust both ways;
/// (transport) a uniquely-tagged SignedPublic root message authored on alice
/// is delivered peer-to-peer and observed live through bob's API; (receiver)
/// bob's ingest accepts it (alice is the active admin in bob's imported card);
/// (sender restart) alice is terminated and respawned on the SAME data dir and
/// comes back healthy with the SAME agent id (identity durable); (durable
/// history) alice's ADR-0023 SQLite store survived the restart and still
/// returns the exact message body under the stable-group scope; (no
/// relay/bridge) each daemon's `/diagnostics/connectivity` reports zero relayed
/// connections and a direct loopback link while the mesh is live.
#[cfg(unix)]
#[test]
#[ignore = "native real-process two-node: needs TTT_X0XD_BINARY (or ../x0x/target/debug/x0xd); run with --ignored --nocapture --test-threads=1"]
fn native_two_node_public_group_durable_history() {
    // ── Process startup ────────────────────────────────────────────────
    // Two fixed loopback QUIC ports so bob can explicitly bootstrap to alice.
    let alice_bind = allocate_unused_udp_port();
    let bob_bind = allocate_unused_udp_port();
    let mut pair = PairGuard {
        alice: NativeDaemonFixture::start_with(FixtureOptions {
            bind_port: alice_bind,
            bootstrap_peers: Vec::new(),
        }),
        bob: NativeDaemonFixture::start_with(FixtureOptions {
            bind_port: bob_bind,
            bootstrap_peers: vec![format!("127.0.0.1:{alice_bind}")],
        }),
    };
    let alice_api = pair.alice.api_base();
    let alice_tok = pair.alice.api_token().to_string();
    let bob_api = pair.bob.api_base();
    let bob_tok = pair.bob.api_token().to_string();
    let alice = authed_client(&alice_tok);
    let bob = authed_client(&bob_tok);
    eprintln!("[two-node] alice={alice_api} bind={alice_bind}; bob={bob_api} bind={bob_bind}");

    // ── Topology / pairing ─────────────────────────────────────────────
    // Mesh via explicit loopback bootstrap, then the supported card/connect
    // flow both ways (mirrors the daemon's own live test sequence).
    let mesh = wait_until(Duration::from_secs(60), || {
        peer_count(&alice, &alice_api) > 0 && peer_count(&bob, &bob_api) > 0
    });
    assert!(mesh, "loopback mesh never formed (bob bootstrap→alice)");

    let alice_id = agent_id(&alice, &alice_api);
    let bob_id = agent_id(&bob, &bob_api);
    assert!(!alice_id.is_empty() && !bob_id.is_empty() && alice_id != bob_id);
    let alice_link = agent_card_link(&alice, &alice_api);
    let bob_link = agent_card_link(&bob, &bob_api);
    let r = rest_post(
        &alice,
        &format!("{alice_api}/agent/card/import"),
        serde_json::json!({"card": bob_link, "trust_level": "Trusted"}),
    );
    assert_eq!(r["ok"], true, "alice agent-card import failed: {r:?}");
    let r = rest_post(
        &bob,
        &format!("{bob_api}/agent/card/import"),
        serde_json::json!({"card": alice_link, "trust_level": "Trusted"}),
    );
    assert_eq!(r["ok"], true, "bob agent-card import failed: {r:?}");
    let r = rest_post(
        &alice,
        &format!("{alice_api}/agents/connect"),
        serde_json::json!({"agent_id": bob_id}),
    );
    assert_eq!(r["ok"], true, "alice connect failed: {r:?}");
    let r = rest_post(
        &bob,
        &format!("{bob_api}/agents/connect"),
        serde_json::json!({"agent_id": alice_id}),
    );
    assert_eq!(r["ok"], true, "bob connect failed: {r:?}");
    let mesh2 = wait_until(Duration::from_secs(30), || {
        peer_count(&alice, &alice_api) > 0 && peer_count(&bob, &bob_api) > 0
    });
    assert!(mesh2, "mesh lost after connect");
    eprintln!("[two-node] paired: alice={alice_id} bob={bob_id}");

    // ── Group: public_open (SignedPublic, OpenJoin, Public read) ───────
    let needle = format!(
        "TWO-NODE-PROOF-pid{}-n{}",
        std::process::id(),
        INSTANCE_TAG.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    );
    let group = rest_post(
        &alice,
        &format!("{alice_api}/groups"),
        serde_json::json!({"name": needle, "description": "two-node durable-history proof", "preset": "public_open"}),
    );
    assert_eq!(
        group["ok"], true,
        "create public_open group failed: {group:?}"
    );
    let local_id = group["group_id"].as_str().unwrap_or_default().to_string();
    assert!(!local_id.is_empty(), "missing group_id: {group:?}");

    // Warmup send primes alice's own public-topic listener BEFORE the tagged
    // send (first-message-on-fresh-topic race, per the daemon's own contract).
    let warm = rest_post(
        &alice,
        &format!("{alice_api}/groups/{local_id}/send"),
        serde_json::json!({"body": "warmup", "kind": "chat"}),
    );
    assert_eq!(warm["ok"], true, "warmup send failed: {warm:?}");

    // The group card carries the STABLE group id — the durable-history scope key.
    let card = rest_get(&alice, &format!("{alice_api}/groups/cards/{local_id}"));
    let stable_id = card["group_id"].as_str().unwrap_or_default().to_string();
    assert!(
        !stable_id.is_empty(),
        "group card missing stable group_id: {card:?}"
    );

    // bob imports the card → bob's stub records alice as active admin AND bob
    // subscribes to the public topic (the receive precondition).
    let imported = rest_post(&bob, &format!("{bob_api}/groups/cards/import"), card);
    assert_eq!(
        imported["ok"], true,
        "bob group-card import failed: {imported:?}"
    );
    // Prime bob's listener, then a bounded settle.
    let _ = rest_get(&bob, &format!("{bob_api}/groups/{stable_id}/messages"));
    std::thread::sleep(Duration::from_secs(1));

    // ── Transport: alice authors the SignedPublic root message ─────────
    let sent = rest_post(
        &alice,
        &format!("{alice_api}/groups/{local_id}/send"),
        serde_json::json!({"body": needle, "kind": "chat"}),
    );
    assert_eq!(sent["ok"], true, "tagged send failed: {sent:?}");
    eprintln!(
        "[two-node] sent tagged needle on alice group {local_id} (stable {stable_id}): {needle}"
    );

    // ── Receiver observation: bob's live API sees the exact message ────
    let observed = wait_until(Duration::from_secs(30), || {
        let msgs = rest_get(&bob, &format!("{bob_api}/groups/{stable_id}/messages"));
        msgs["messages"].as_array().is_some_and(|ms| {
            ms.iter()
                .any(|m| m["body"].as_str() == Some(needle.as_str()))
        })
    });
    assert!(
        observed,
        "bob never observed alice's SignedPublic message (peer-to-peer delivery failed)"
    );
    eprintln!("[two-node] bob observed the SignedPublic message live (peer-to-peer): {needle}");

    // ── No relay / bridge on the path (scoped to our daemons) ──────────
    // Queried while the loopback mesh is live: the alice↔bob link must be a
    // DIRECT connection with zero relayed connections — observable proof that
    // no relay/bridge process or URL carried the SignedPublic message. This is
    // daemon-scoped (the daemon's own transport counters), not a host-wide
    // process scan, so unrelated daemons or stray binaries cannot trip it.
    let (a_direct, a_relayed) = connection_split(&alice, &alice_api);
    let (b_direct, b_relayed) = connection_split(&bob, &bob_api);
    assert_eq!(
        a_relayed, 0,
        "alice used a relayed connection — relay on the path (direct={a_direct}, relayed={a_relayed})"
    );
    assert_eq!(
        b_relayed, 0,
        "bob used a relayed connection — relay on the path (direct={b_direct}, relayed={b_relayed})"
    );
    assert!(
        a_direct + b_direct > 0,
        "no direct connection between alice and bob — transport not peer-to-peer"
    );
    eprintln!("[two-node] transport is peer-to-peer: alice direct={a_direct} relayed={a_relayed}; bob direct={b_direct} relayed={b_relayed}");

    // ── Sender restart: terminate alice, respawn on the SAME data dir ──
    pair.alice.restart();
    let alice_api2 = pair.alice.api_base();
    let alice_tok2 = pair.alice.api_token().to_string();
    assert_ne!(
        alice_api2,
        String::new(),
        "alice did not come back after restart"
    );
    let alice2 = authed_client(&alice_tok2);
    // Identity is durable: same agent id across the restart.
    let alice_id_after = agent_id(&alice2, &alice_api2);
    assert_eq!(
        alice_id_after, alice_id,
        "alice agent id changed across restart — identity not durable"
    );
    eprintln!("[two-node] alice restarted on same data dir; api={alice_api2} agent_id preserved");

    // ── Durable history: the exact message survived the restart ────────
    // Queried by the stable-group scope; payload is base64 of the UTF-8 body.
    let found = wait_until(Duration::from_secs(20), || {
        let h = rest_get(
            &alice2,
            &format!("{alice_api2}/history?scope=group:{stable_id}"),
        );
        h["records"].as_array().is_some_and(|rows| {
            rows.iter().any(|r| {
                r["payload"]
                    .as_str()
                    .and_then(decode_b64_payload)
                    .is_some_and(|s| s == needle)
            })
        })
    });
    assert!(
        found,
        "alice durable history lost the tagged message across restart (scope=group:{stable_id})"
    );

    eprintln!("[two-node] PASS: two isolated nodes paired over loopback, exchanged a SignedPublic message, alice restarted on the same data dir, and durable history still holds the exact body — transport peer-to-peer, no relay/bridge involved.");
}

/// Per-process monotonic tag so the tagged needle is unique across test runs.
static INSTANCE_TAG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

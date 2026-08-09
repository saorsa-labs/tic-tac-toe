use super::company::CompanyInstantiationError;
use super::instances::{
    clear_active_company_instance_if_matches, company_instance_dir, existing_active_instance,
    scan_company_instances, set_active_company_instance,
};
use super::lifecycle::{company_run_cancelled, lifecycle_is_complete, mark_company_resumable};
use super::test_injection;
use crate::app_state::build_app_state;
use crate::company_template::instantiate::{
    InstanceManifest, JsonManifestSink, ManifestSink, ManifestStatus,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Two concurrent `instantiate_company_template` calls cannot both pass the
/// active-instance scan and reserve a slot: `company_instantiate_lock`
/// spans the scan AND the reservation write, so exactly one is accepted.
///
/// Defends the single-active-company invariant — a missing lock, or a scan
/// that does not observe a just-written reservation, would let two callers
/// through. Uses the real scan, sink, and lock discipline against a temp
/// instances root. (The orchestrator command itself needs a live
/// `AppHandle`, so the critical section is reproduced over the real
/// primitives it composes.)
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_instantiate_reserves_exactly_one_active_instance() {
    let _scoped = test_injection::ScopedRoot::new();
    let state = Arc::new(build_app_state());
    let accepted = Arc::new(AtomicUsize::new(0));
    let refused = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for i in 0..16u64 {
        let state = state.clone();
        let accepted = accepted.clone();
        let refused = refused.clone();
        handles.push(tokio::spawn(async move {
            // The production critical section: lock → scan → reserve or
            // refuse. The lock is held across both so a concurrent caller
            // observes the reservation; no await inside.
            let _guard = state
                .company_instantiate_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if existing_active_instance().is_some() {
                refused.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let instance_id = format!("race-company-{i}");
            let manifest_path = company_instance_dir(&instance_id)
                .expect("instance dir resolves under the scoped root")
                .join("manifest.json");
            JsonManifestSink::new(&manifest_path)
                .save(&InstanceManifest::fresh(
                    &instance_id,
                    "software-dev-and-sales",
                ))
                .expect("reserve fresh manifest");
            accepted.fetch_add(1, Ordering::SeqCst);
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(
        accepted.load(Ordering::SeqCst),
        1,
        "exactly one Company instance may be reserved under contention"
    );
    assert_eq!(refused.load(Ordering::SeqCst), 15);
    // The winner is observable as the single active instance on disk.
    let active = existing_active_instance();
    assert!(
        active
            .as_ref()
            .is_some_and(|id| id.starts_with("race-company-")),
        "the reserved instance is the single active one, got {active:?}"
    );
}

/// A cancelled instance is terminal: the single-active gate ignores it, so
/// a new company is NOT refused while only a cancelled instance remains on
/// disk. This is the reconcile/gate skip at the scan level.
#[test]
fn cancelled_instance_is_not_treated_as_active() {
    let _scoped = test_injection::ScopedRoot::new();
    let instance_id = "cancelled-company-1700000000001";
    let manifest_path = company_instance_dir(instance_id)
        .expect("instance dir resolves under the scoped root")
        .join("manifest.json");
    let mut manifest = InstanceManifest::fresh(instance_id, "software-dev-and-sales");
    manifest.status = ManifestStatus::Cancelled;
    manifest.cancelled_at = Some(1_700_000_000_001);
    JsonManifestSink::new(&manifest_path)
        .save(&manifest)
        .expect("persist cancelled tombstone");

    assert!(
        existing_active_instance().is_none(),
        "a cancelled instance must not block a new company"
    );
}

#[test]
fn lifecycle_complete_gate_requires_every_post_phase_and_a_run_id() {
    // The orchestrator binds an active company (set_active_company_instance)
    // and reconcile rebinds ONLY when lifecycle_is_complete is true. The gate
    // must be false unless the manifest is Complete AND carries a durable
    // run_id AND every required post-phase (identities, membership,
    // symphony_bind) is checkpointed — so a crash between phases, or a
    // Complete marker persisted without a run_id, can NEVER bind an active
    // company. Each branch removes exactly one requirement.
    use crate::company_template::instantiate::post_phase_step;

    let instance = "gate-co";
    let mut base = InstanceManifest::fresh(instance, "software-dev-and-sales");
    base.status = ManifestStatus::Complete;
    base.run_id = Some("run-1".to_string());
    base.completed
        .push(post_phase_step(instance, "identities", "identities"));
    base.completed
        .push(post_phase_step(instance, "membership", "membership"));
    base.completed
        .push(post_phase_step(instance, "symphony_bind", "symphony_bind"));

    // Fully complete → eligible to bind active.
    assert!(lifecycle_is_complete(&base));

    // Removing any single requirement → never eligible.
    let mut r = base.clone();
    r.status = ManifestStatus::Resumable;
    assert!(!lifecycle_is_complete(&r), "Resumable is not complete");

    let mut r = base.clone();
    r.status = ManifestStatus::InProgress;
    assert!(!lifecycle_is_complete(&r), "InProgress is not complete");

    let mut r = base.clone();
    r.run_id = None;
    assert!(
        !lifecycle_is_complete(&r),
        "Complete without a durable run_id must never bind active"
    );

    let mut r = base.clone();
    r.completed.retain(|c| c.kind != "identities");
    assert!(
        !lifecycle_is_complete(&r),
        "missing the identities post-phase must never bind active"
    );

    let mut r = base.clone();
    r.completed.retain(|c| c.kind != "membership");
    assert!(
        !lifecycle_is_complete(&r),
        "missing the membership post-phase must never bind active"
    );

    let mut r = base.clone();
    r.completed.retain(|c| c.kind != "symphony_bind");
    assert!(
        !lifecycle_is_complete(&r),
        "missing the symphony_bind post-phase must never bind active"
    );

    // A Cancelled manifest is never complete (cancel is terminal, never active).
    let mut cancelled = base.clone();
    cancelled.status = ManifestStatus::Cancelled;
    assert!(
        !lifecycle_is_complete(&cancelled),
        "a cancelled manifest must never be bound active"
    );
}

#[test]
fn any_non_cancelled_manifest_blocks_fresh_instantiate_and_newest_wins() {
    // existing_active_instance() is the single-active gate instantiate runs:
    // a fresh company is refused while ANY non-cancelled instance persists
    // (Complete OR partial). Only a Cancelled tombstone frees the slot.
    // Among several non-cancelled the gate names the newest by epoch, so the
    // operator error points at the instance actually blocking — not a stale
    // one. (cancelled_instance_is_not_treated_as_active pins the Cancelled
    // case; this pins the three blocking statuses + newest-wins.)
    let _scoped = test_injection::ScopedRoot::new();

    let persist = |id: &str, status: ManifestStatus| {
        let path = company_instance_dir(id)
            .expect("instance dir resolves under the scoped root")
            .join("manifest.json");
        let mut m = InstanceManifest::fresh(id, "software-dev-and-sales");
        m.status = status;
        JsonManifestSink::new(&path)
            .save(&m)
            .expect("persist manifest");
    };

    // An in_progress reservation blocks.
    persist("co-inprogress-1700000000001", ManifestStatus::InProgress);
    assert_eq!(
        existing_active_instance().as_deref(),
        Some("co-inprogress-1700000000001"),
        "an in_progress reservation must block a fresh instantiate"
    );

    // A resumable (partial) instance also blocks; newest epoch wins.
    persist("co-resumable-1700000000002", ManifestStatus::Resumable);
    assert_eq!(
        existing_active_instance().as_deref(),
        Some("co-resumable-1700000000002"),
        "a resumable instance blocks and the newest epoch wins"
    );

    // A complete instance blocks too (single-active until cancelled).
    persist("co-complete-1700000000003", ManifestStatus::Complete);
    assert_eq!(
        existing_active_instance().as_deref(),
        Some("co-complete-1700000000003"),
        "a complete instance blocks until cancelled; newest epoch wins"
    );

    // A cancelled tombstone never blocks — the newest non-cancelled still does.
    persist("co-cancelled-1700000000004", ManifestStatus::Cancelled);
    assert_eq!(
        existing_active_instance().as_deref(),
        Some("co-complete-1700000000003"),
        "a cancelled instance never blocks; the newest non-cancelled still does"
    );
}

#[test]
fn reconcile_rebind_target_is_the_newest_lifecycle_complete_instance() {
    // reconcile_companies (re)binds the supervised daemon to the single
    // NEWEST lifecycle-complete instance and marks it active — never an
    // incomplete or cancelled one. lifecycle_is_complete is the gate (pinned
    // above); this exercises the selection over a real on-disk set via the
    // same primitives reconcile uses (scan_company_instances +
    // lifecycle_is_complete): exactly the lifecycle-complete instances are
    // eligible, the newest epoch wins, and incomplete/cancelled instances are
    // never rebind targets.
    use crate::company_template::instantiate::post_phase_step;

    enum State {
        Complete,
        Incomplete,
        Cancelled,
    }
    let _scoped = test_injection::ScopedRoot::new();
    let persist = |id: &str, state: State| {
        let path = company_instance_dir(id)
            .expect("instance dir resolves under the scoped root")
            .join("manifest.json");
        let mut m = InstanceManifest::fresh(id, "software-dev-and-sales");
        match state {
            State::Complete => {
                m.status = ManifestStatus::Complete;
                m.run_id = Some(format!("run-{id}"));
                m.completed
                    .push(post_phase_step(id, "identities", "identities"));
                m.completed
                    .push(post_phase_step(id, "membership", "membership"));
                m.completed
                    .push(post_phase_step(id, "symphony_bind", "symphony_bind"));
            }
            State::Incomplete => m.status = ManifestStatus::Resumable,
            State::Cancelled => {
                m.status = ManifestStatus::Cancelled;
                m.cancelled_at = Some(1);
            }
        }
        JsonManifestSink::new(&path)
            .save(&m)
            .expect("persist manifest");
    };

    persist("recon-old-complete-1700000000001", State::Complete);
    persist("recon-new-incomplete-1700000000002", State::Incomplete);
    persist("recon-new-complete-1700000000003", State::Complete);
    persist("recon-cancelled-1700000000004", State::Cancelled);

    // Eligible set = exactly the lifecycle-complete instances.
    let mut eligible: Vec<String> = scan_company_instances()
        .into_iter()
        .filter(|s| {
            JsonManifestSink::new(&s.manifest_path)
                .load()
                .is_ok_and(|m| lifecycle_is_complete(&m))
        })
        .map(|s| s.instance_id)
        .collect();
    eligible.sort();
    assert_eq!(
        eligible,
        vec![
            "recon-new-complete-1700000000003".to_string(),
            "recon-old-complete-1700000000001".to_string(),
        ],
        "only lifecycle-complete instances are eligible; incomplete/cancelled are skipped"
    );

    // The rebind target is the newest eligible by epoch (reconcile's order).
    let target = scan_company_instances()
        .into_iter()
        .filter(|s| {
            JsonManifestSink::new(&s.manifest_path)
                .load()
                .is_ok_and(|m| lifecycle_is_complete(&m))
        })
        .max_by_key(|s| s.epoch)
        .map(|s| s.instance_id);
    assert_eq!(
        target.as_deref(),
        Some("recon-new-complete-1700000000003"),
        "rebind targets the newest lifecycle-complete instance"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_resume_and_instantiate_share_one_lock_order_without_deadlock() {
    // Both commands take company_run_lock (outermost) then
    // company_instantiate_lock (nested) — the SAME order — so mixed
    // resume/instantiate contention cannot ABBA. This reproduces the real
    // critical sections over the production locks: a resumable instance is
    // pre-seeded, then resume-validations and instantiate-reservations race,
    // each under run→instantiate. Asserts no deadlock (timeout-bounded) and
    // that single-active holds — no instantiate reserves over the resumable.
    use std::time::Duration;
    let _scoped = test_injection::ScopedRoot::new();
    let state = Arc::new(build_app_state());

    let instance = "race-resume-1700000000000";
    let path = company_instance_dir(instance)
        .expect("instance dir resolves under the scoped root")
        .join("manifest.json");
    let mut manifest = InstanceManifest::fresh(instance, "software-dev-and-sales");
    manifest.status = ManifestStatus::Resumable;
    JsonManifestSink::new(&path)
        .save(&manifest)
        .expect("seed resumable instance");

    let reservations = Arc::new(AtomicUsize::new(0));
    let resumes_validated = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for i in 0u64..16 {
        let state = state.clone();
        let path = path.clone();
        let reservations = reservations.clone();
        let resumes_validated = resumes_validated.clone();
        handles.push(tokio::spawn(async move {
            // Canonical order: run-outermost → instantiate-nested (both cmds).
            let _run_guard = state.company_run_lock.lock().await;
            let _guard = state
                .company_instantiate_lock
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if i % 2 == 0 {
                // resume critical section: reuse the EXACT existing id.
                let m = JsonManifestSink::new(&path)
                    .load()
                    .expect("load for resume");
                assert_eq!(
                    m.instance_id, instance,
                    "resume reuses the exact id, never mints"
                );
                resumes_validated.fetch_add(1, Ordering::SeqCst);
            } else if existing_active_instance().is_none() {
                // instantiate critical section: single-active refuses over the
                // existing non-cancelled instance (so this never reserves).
                let id = format!("race-instantiate-{i}");
                let p = company_instance_dir(&id)
                    .expect("instance dir resolves under the scoped root")
                    .join("manifest.json");
                JsonManifestSink::new(&p)
                    .save(&InstanceManifest::fresh(&id, "software-dev-and-sales"))
                    .expect("reserve");
                reservations.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    // No deadlock: every contender finishes well within the deadline. A real
    // ABBA inverse would hang here and trip the timeout.
    let joined = tokio::time::timeout(Duration::from_secs(10), async {
        for h in handles {
            h.await.expect("task panicked");
        }
    })
    .await;
    assert!(
        joined.is_ok(),
        "resume+instantiate deadlocked (did not complete in 10s)"
    );

    // Single-active honored: the resumable instance blocks every reservation.
    assert_eq!(
        reservations.load(Ordering::SeqCst),
        0,
        "no new company may be reserved while a resumable one exists"
    );
    assert_eq!(resumes_validated.load(Ordering::SeqCst), 8);
    assert_eq!(existing_active_instance().as_deref(), Some(instance));
}

#[test]
fn mark_resumable_stays_silent_when_the_manifest_is_already_cancelled() {
    // When the manifest is already Cancelled, mark_company_resumable must
    // NOT push an error or overwrite the tombstone (cancel is terminal; the
    // sink guard refusal is expected, not a provisioning failure).
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    let sink = JsonManifestSink::new(&path);
    let mut manifest = InstanceManifest::fresh("silent-co", "software-dev-and-sales");
    manifest.status = ManifestStatus::Cancelled;
    sink.save(&manifest).expect("seed cancelled tombstone");

    let mut errors: Vec<CompanyInstantiationError> = Vec::new();
    mark_company_resumable(&mut errors, &sink);
    assert!(
        errors.is_empty(),
        "a cancelled tombstone must not surface a resumable error"
    );
    assert_eq!(
        sink.load().unwrap().status,
        ManifestStatus::Cancelled,
        "the tombstone is untouched"
    );
}

#[test]
fn cancel_during_the_final_bind_cannot_leave_the_instance_active_or_complete() {
    // run_company_lifecycle binds active only at the very end, then re-reads
    // durable state: a cancel that lands during the bind must undo the
    // binding (clear_active_company_instance_if_matches) so the instance can
    // never end up active or Complete. Reproduce the post-bind reload over
    // the real primitives, then prove clearing never clobbers a different
    // instance.
    let _scoped = test_injection::ScopedRoot::new();
    let state = Arc::new(build_app_state());
    let instance = "bind-co-1700000000001";
    let path = company_instance_dir(instance)
        .expect("instance dir resolves under the scoped root")
        .join("manifest.json");
    let sink = JsonManifestSink::new(&path);

    // The lifecycle persisted a Complete manifest + run_id and bound active.
    let mut manifest = InstanceManifest::fresh(instance, "software-dev-and-sales");
    manifest.status = ManifestStatus::Complete;
    manifest.run_id = Some("run-bind".to_string());
    manifest.cancel_generation = 0;
    sink.save(&manifest).expect("persist complete");
    set_active_company_instance(&state, instance);
    assert_eq!(
        *state
            .active_company_instance
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
        Some(instance.to_string()),
        "the lifecycle bound the instance active"
    );

    let observed = 0u64;
    assert!(
        !company_run_cancelled(&sink, instance, observed),
        "no cancel before the bind completes"
    );

    // A cancel tombstone lands during the bind (generation bumped to 1).
    let mut tombstone = manifest.clone();
    tombstone.status = ManifestStatus::Cancelled;
    tombstone.cancel_generation = 1;
    sink.save(&tombstone).expect("persist cancel tombstone");

    // The post-bind reload observes the cancel and undoes ONLY this binding.
    assert!(company_run_cancelled(&sink, instance, observed));
    clear_active_company_instance_if_matches(&state, instance);
    assert_eq!(
        *state
            .active_company_instance
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
        None,
        "cancel during the bind must unbind the just-bound instance"
    );

    // The tombstone is terminal: lifecycle_is_complete is false — the
    // instance can never be reported active/Complete again.
    assert!(
        !lifecycle_is_complete(&sink.load().expect("reload tombstone")),
        "a cancelled instance is never lifecycle-complete"
    );

    // clear_active_company_instance_if_matches never clobbers a DIFFERENT
    // instance (e.g. one the reconciler bound meanwhile).
    set_active_company_instance(&state, "other-co-1700000000002");
    clear_active_company_instance_if_matches(&state, instance);
    assert_eq!(
        *state
            .active_company_instance
            .lock()
            .unwrap_or_else(|p| p.into_inner()),
        Some("other-co-1700000000002".to_string()),
        "clearing must not clobber a different instance's binding"
    );
}

//! M4 hybrid-slice tests for the `company_template` backend — runtime slice.
//!
//! Runtime/provisioning half of the M4 hybrid contract, split from
//! `m4_hybrid_tests.rs` so each file stays under the desktop file-size gate.
//! Targets the confirmed seams exercised *after* planning:
//!   - Resumable, checkpointed, fail-closed native x0xd provisioning.
//!   - Durable manifest lifecycle (InProgress/Resumable/Cancelled tombstones).
//!   - The supported-runtime contract gate (single harness + primary queue).
//!   - **Zero relay events**: the module never reaches a Nostr relay emit.
//!
//! No production files are edited by this test module. Wired in by the module
//! owner via `#[cfg(test)] mod m4_hybrid_runtime_tests;` (the same convention
//! as `m4_hybrid_tests.rs`).

use super::instantiate::{
    instantiate_company, InMemorySink, JsonManifestSink, ManifestSink, ManifestStatus,
};
use super::parse::{
    parse_company_template, parse_lenient, validate_supported_contract, ParseError, ValidationIssue,
};
use super::plan::{plan_instantiation, PlanStep};
use super::provisioner::RecordingProvisioner;
use super::spec::{CompanyTemplate, GroupVisibility, RoleSpec, TaskListSpec};

// ─── Fixtures ────────────────────────────────────────────────────────────────

/// A minimal, valid `software-dev-and-sales`-shaped template exercising every
/// validated relation: two `private_secure` groups, one `public_open` group,
/// roles that staff them, typed Symphony stores (one group-scoped backlog, one
/// company-shared proofs store), and a shared task list.
const SOFTWARE_DEV_AND_SALES_TOML: &str = r#"
id = "software-dev-and-sales"
name = "Software Dev & Sales"
description = "Engineering plus sales, dogfooded on Saorsa."
version = "1"

[[groups]]
id = "engineering"
name = "Engineering"
visibility = "private_secure"
purpose = "Build the product."
members = ["staff-engineer"]

[[groups]]
id = "sales"
name = "Sales"
visibility = "private_secure"
members = ["account-exec"]

[[groups]]
id = "all-hands"
name = "All-Hands"
visibility = "public_open"
members = ["staff-engineer", "account-exec"]

[[roles]]
id = "staff-engineer"
name = "Staff Engineer"
harness = "codex"
skills = ["code-review", "git-ops"]
groups = ["engineering", "all-hands"]

[[roles]]
id = "account-exec"
name = "Account Executive"
harness = "claude"
groups = ["sales", "all-hands"]

[[stores]]
id = "engineering-backlog"
name = "Engineering Backlog"
kind = "backlog"
policy = "append_only"
group = "engineering"

[[stores]]
id = "company-proofs"
name = "Company Proofs"
kind = "proofs"

[[task_lists]]
id = "sales-pipeline"
name = "Sales Pipeline"
group = "sales"
"#;

fn parsed_dev_sales() -> CompanyTemplate {
    parse_company_template(SOFTWARE_DEV_AND_SALES_TOML).expect("fixture template must parse")
}

// ─── #7 Resumable native provisioning ──────────────────────────────────────

#[tokio::test]
async fn instantiation_is_checkpointed_and_idempotent() {
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "resumable-company");
    let provisioner = RecordingProvisioner::new();
    let sink = InMemorySink::new();

    let first = instantiate_company(&plan, &provisioner, &sink).await;
    assert!(first.is_complete());
    assert_eq!(provisioner.group_calls(), 3);
    assert_eq!(provisioner.store_calls(), 2);
    assert_eq!(provisioner.task_list_calls(), 1);
    let calls = provisioner.total_calls();

    let second = instantiate_company(&plan, &provisioner, &sink).await;
    assert!(second.is_complete());
    assert_eq!(provisioner.total_calls(), calls);
    let manifest = sink.load().expect("manifest checkpoint");
    // The provisioning executor persists InProgress (NEVER Complete); full
    // lifecycle completion is owned by the orchestrator. The OUTCOME status is
    // Complete (provisioning phase done → orchestrator advances to the post-
    // phases), but the durable manifest on disk is InProgress.
    assert_eq!(manifest.status, ManifestStatus::InProgress);
}

#[tokio::test]
async fn provisioning_failure_leaves_a_resumable_manifest() {
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "partial-company");
    let provisioner = RecordingProvisioner::new();
    provisioner.fail_next(1);
    let sink = InMemorySink::new();

    let outcome = instantiate_company(&plan, &provisioner, &sink).await;
    assert!(!outcome.is_complete());
    assert_eq!(outcome.errors().len(), 1);
    assert_eq!(
        sink.load().expect("resumable checkpoint").status,
        ManifestStatus::Resumable
    );
}

#[tokio::test]
async fn resume_skips_completed_steps_and_finishes_the_remaining_phases() {
    // A partial run crashed after the first group was provisioned + checkpointed.
    // Re-entering instantiate_company with the same manifest (the executor-level
    // resume the orchestrator's resume_company_instance drives) must SKIP every
    // completed step, provision ONLY the remaining ones, and reach Complete —
    // the durable manifest makes the resume idempotent. Defends the contract:
    // "explicit resume reuses idempotent completed checkpoints" and "resume
    // finishes remaining phases and yields Complete."
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "resume-company");
    let provisioner = RecordingProvisioner::new();
    let sink = InMemorySink::new();

    // Seed durable state: the first plan step (CreateGroup "engineering") was
    // already provisioned + checkpointed before the crash.
    let first_step = &plan.steps[0];
    assert!(matches!(first_step, PlanStep::CreateGroup { .. }));
    {
        let mut manifest =
            super::instantiate::InstanceManifest::fresh(&plan.instance_id, &plan.template_id);
        manifest.completed.push(super::instantiate::CompletedStep {
            key: first_step.key().to_string(),
            kind: first_step.kind_label().to_string(),
            local_id: first_step.local_target().map(|s| s.to_string()),
            resource_id: "engineering-already-realized".to_string(),
            created: false,
        });
        sink.save(&manifest).expect("seed partial manifest");
    }

    let outcome = instantiate_company(&plan, &provisioner, &sink).await;

    // Resume reached Complete.
    assert!(
        outcome.is_complete(),
        "resume must finish the remaining phases, got {:?}",
        outcome.status
    );
    // The already-completed group was SKIPPED: only the 2 remaining groups were
    // provisioned (3 total − 1 seeded), never re-creating the realized resource.
    assert_eq!(
        provisioner.group_calls(),
        2,
        "resume must skip the completed group and provision the rest"
    );
    // The skipped step surfaces with its durable resource id and created == false.
    let skipped = outcome
        .steps
        .iter()
        .find(|s| s.kind == "group" && s.local_id.as_deref() == Some("engineering"))
        .expect("seeded group step present in the resume outcome");
    assert!(skipped.skipped, "the checkpointed step must be skipped");
    assert!(!skipped.created);
    assert_eq!(
        skipped.resource_id.as_deref(),
        Some("engineering-already-realized"),
        "the durable resource id is reused verbatim"
    );
    // The durable manifest is InProgress after provisioning: the executor never
    // persists Complete (the orchestrator owns that single point), but the
    // outcome status is Complete (provisioning finished → resume advances).
    assert_eq!(
        sink.load().expect("manifest after resume").status,
        ManifestStatus::InProgress
    );
}

#[test]
fn template_count_helpers_cover_the_product_picker_summary() {
    let template = parse_lenient(SOFTWARE_DEV_AND_SALES_TOML).expect("lenient parse");
    assert_eq!(template.group_count(), 3);
    assert_eq!(template.role_count(), 2);
    assert_eq!(template.store_count(), 2);
    assert_eq!(template.task_list_count(), 1);

    let manifest = JsonManifestSink::new("/tmp/x0x-company/manifest.json");
    assert_eq!(
        manifest.path(),
        std::path::Path::new("/tmp/x0x-company/manifest.json")
    );
}

#[cfg(unix)]
#[test]
fn manifest_sink_restricts_file_and_dir_permissions() {
    use super::instantiate::{InstanceManifest, JsonManifestSink, ManifestSink};
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    let sink = JsonManifestSink::new(&path);

    let manifest = InstanceManifest::fresh("perm-test", "software-dev-and-sales");
    sink.save(&manifest).expect("save manifest");

    let file_mode = std::fs::metadata(&path).unwrap().permissions().mode();
    let dir_mode = std::fs::metadata(dir.path()).unwrap().permissions().mode();
    // Owner-only: 0o600 for files, 0o700 for directories.
    assert_eq!(
        file_mode & 0o777,
        0o600,
        "manifest file must be 0600, got {:o}",
        file_mode & 0o777
    );
    assert_eq!(
        dir_mode & 0o777,
        0o700,
        "instance dir must be 0700, got {:o}",
        dir_mode & 0o777
    );
}

#[test]
fn manifest_round_trips_run_id_for_durable_run_map() {
    use super::instantiate::{InstanceManifest, JsonManifestSink, ManifestSink};

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    let sink = JsonManifestSink::new(&path);

    // Fresh manifest has no run_id.
    let mut manifest = InstanceManifest::fresh("durability-test", "software-dev-and-sales");
    assert!(manifest.run_id.is_none());
    sink.save(&manifest).expect("save");

    // Simulate the orchestrator persisting the run_id after creating the run.
    manifest.run_id = Some("symphony-issue-42".into());
    sink.save(&manifest).expect("save with run_id");

    // Reload: run_id is durable for runId→instanceId reconstruction.
    let loaded = sink.load().expect("load");
    assert_eq!(loaded.instance_id, "durability-test");
    assert_eq!(loaded.run_id.as_deref(), Some("symphony-issue-42"));

    // A manifest written without run_id (older format) still loads with None.
    std::fs::write(
        &path,
        r#"{"instance_id":"old","template_id":"t","status":"complete","completed":[]}"#,
    )
    .unwrap();
    let legacy = sink.load().expect("load legacy");
    assert!(
        legacy.run_id.is_none(),
        "missing run_id field defaults to None"
    );
}

// ─── #9 Fail-closed lifecycle gates ─────────────────────────────────────────
//
// The Company instantiation executor is fail-closed at every durable boundary:
// a checkpoint (manifest save) failure must HALT provisioning and never claim
// progress it could not persist. The executor NEVER persists Complete — full
// lifecycle completion (post-phases + run_id) is the orchestrator's single
// atomic point. Together these defend the "durable + resumable, never silently
// wrong" M4 contract.

/// Manifest sink whose `save` always fails — exercises the fail-closed
/// checkpoint path: the resource was created but cannot be recorded.
struct FailingManifestSink;

impl ManifestSink for FailingManifestSink {
    fn load(
        &self,
    ) -> Result<super::instantiate::InstanceManifest, super::instantiate::ManifestError> {
        Err(super::instantiate::ManifestError::NotFound)
    }
    fn save(
        &self,
        _: &super::instantiate::InstanceManifest,
    ) -> Result<(), super::instantiate::ManifestError> {
        Err(super::instantiate::ManifestError::Io(
            "simulated durable-storage failure".into(),
        ))
    }
}

#[tokio::test]
async fn checkpoint_failure_halts_provisioning_and_claims_no_unpersisted_progress() {
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "checkpoint-fail-company");
    let provisioner = RecordingProvisioner::new();
    // Failing sink: every checkpoint (manifest save) fails.
    let outcome = instantiate_company(&plan, &provisioner, &FailingManifestSink).await;

    // Fail-closed: the run is resumable, never complete.
    assert_eq!(outcome.status, ManifestStatus::Resumable);
    // The checkpoint failure is surfaced distinctly from per-step errors.
    assert!(
        outcome
            .manifest_error
            .as_ref()
            .is_some_and(|m| m.contains("manifest checkpoint failed")),
        "expected checkpoint failure, got {:?}",
        outcome.manifest_error
    );

    // Provisioning HALTED at the first step: only one group call, no stores or
    // task lists. A bug that swallowed the checkpoint error would provision on.
    assert_eq!(provisioner.group_calls(), 1);
    assert_eq!(provisioner.store_calls(), 0);
    assert_eq!(provisioner.task_list_calls(), 0);

    // Exactly one step attempted — the one whose checkpoint failed.
    assert_eq!(outcome.steps.len(), 1);
    let step = &outcome.steps[0];
    assert_eq!(step.kind, "group");
    // The resource WAS created but we could not persist it, so the outcome
    // never claims it (created == false, no resource_id, error surfaced).
    assert!(
        !step.created,
        "must not claim a resource it could not checkpoint"
    );
    assert!(step.resource_id.is_none());
    assert!(step
        .error
        .as_ref()
        .is_some_and(|m| m.contains("manifest checkpoint failed")));
    assert!(!step.skipped);
}

#[tokio::test]
async fn provisioning_success_persists_in_progress_never_complete() {
    // The provisioning executor NEVER persists Complete: full lifecycle
    // completion (identities + membership + symphony bind + run_id) is owned by
    // the orchestrator, which marks Complete atomically only after every post-
    // phase. A successful provisioning run therefore leaves the durable manifest
    // InProgress (every step already checkpointed), while the OUTCOME status is
    // Complete so the orchestrator advances to the post-phases. This pins the
    // single point that could have falsely claimed Complete before the fix.
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "inprogress-company");
    let provisioner = RecordingProvisioner::new();
    let sink = InMemorySink::new();

    let outcome = instantiate_company(&plan, &provisioner, &sink).await;

    // The outcome signals provisioning-phase completion so the orchestrator
    // advances — and records no checkpoint failure.
    assert_eq!(outcome.status, ManifestStatus::Complete);
    assert!(outcome.manifest_error.is_none());

    // ...but the durable manifest is InProgress, NEVER Complete.
    let manifest = sink.load().expect("manifest checkpoint");
    assert_eq!(
        manifest.status,
        ManifestStatus::InProgress,
        "the executor must not persist Complete; the orchestrator owns that"
    );
    assert!(!manifest.is_complete());
    // Every provisioning step is checkpointed (durable for a resume).
    assert_eq!(manifest.completed.len(), plan.steps.len());
}

#[test]
fn cancelled_tombstone_is_durable_for_reconcile_skip() {
    use super::instantiate::{InstanceManifest, JsonManifestSink, ManifestSink};

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    let sink = JsonManifestSink::new(&path);

    // The cancel path writes a Cancelled tombstone with cancelled_at.
    let mut manifest = InstanceManifest::fresh("cancelled-company", "software-dev-and-sales");
    manifest.status = ManifestStatus::Cancelled;
    manifest.cancelled_at = Some(1_700_000_000_000);
    sink.save(&manifest).expect("persist cancel tombstone");

    // A restart reloads the tombstone: Cancelled survives, so boot
    // reconciliation and the single-active gate skip this instance rather than
    // resuming or treating it as active.
    let reloaded = sink.load().expect("reload tombstone");
    assert_eq!(reloaded.status, ManifestStatus::Cancelled);
    assert_eq!(reloaded.cancelled_at, Some(1_700_000_000_000));

    // Wire shape (parsed, formatter-agnostic): the status token the frontend
    // status display and the reconcile/gate filters read is exactly
    // "cancelled", and cancelled_at is present only on a cancelled tombstone —
    // distinguishing a real cancel from a stale in-progress run.
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["status"].as_str(), Some("cancelled"));
    assert_eq!(on_disk["cancelled_at"].as_u64(), Some(1_700_000_000_000));
}

#[tokio::test]
async fn a_cancelled_manifest_is_terminal_for_the_executor() {
    // Cancelled is terminal at the executor too: instantiate_company never
    // resumes or replays a cancelled instance — it returns Cancelled immediately
    // and provisions nothing, even when completed steps predate the cancel. This
    // is the in-process half of "cancel is terminal"; the durable overwrite guard
    // is pinned in the next test.
    let template = parsed_dev_sales();
    let plan = plan_instantiation(&template, "cancelled-exec-company");
    let provisioner = RecordingProvisioner::new();
    let sink = InMemorySink::new();

    // Seed a Cancelled tombstone carrying one completed step.
    let mut manifest =
        super::instantiate::InstanceManifest::fresh(&plan.instance_id, &plan.template_id);
    manifest.status = ManifestStatus::Cancelled;
    manifest.cancel_generation = 1;
    manifest.completed.push(super::instantiate::CompletedStep {
        key: plan.steps[0].key().to_string(),
        kind: "group".to_string(),
        local_id: Some("engineering".to_string()),
        resource_id: "engineering-realized".to_string(),
        created: true,
    });
    sink.save(&manifest).expect("seed cancelled tombstone");

    let outcome = instantiate_company(&plan, &provisioner, &sink).await;

    assert_eq!(outcome.status, ManifestStatus::Cancelled);
    // Zero provisioning: the executor honored the tombstone and did no work.
    assert_eq!(provisioner.total_calls(), 0);
    assert!(
        outcome.steps.is_empty(),
        "a cancelled resume performs no step work"
    );
    assert!(outcome.workflow_md.is_none());
    assert!(outcome.symphony_config_toml.is_none());
}

#[test]
fn cancelled_tombstone_cannot_be_overwritten_and_re_cancel_is_idempotent() {
    // The durable sink guard is the hard backstop for cooperative cancellation:
    // once a Cancelled tombstone is persisted, NO non-Cancelled save (a stale
    // executor that did not observe the generation bump) can clobber it — it
    // fails with ManifestError::Cancelled. Re-saving Cancelled over Cancelled is
    // allowed (cancel is idempotent). Exercised against the production
    // JsonManifestSink — the same guard the InMemorySink mirrors for the
    // executor tests above.
    use super::instantiate::{InstanceManifest, JsonManifestSink, ManifestError, ManifestSink};

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("manifest.json");
    let sink = JsonManifestSink::new(&path);

    // 1. Persist the Cancelled tombstone (cancel_generation bumped, like
    //    cancel_company_run does before cleanup).
    let mut tombstone = InstanceManifest::fresh("cancel-guard", "software-dev-and-sales");
    tombstone.status = ManifestStatus::Cancelled;
    tombstone.cancel_generation = 1;
    tombstone.cancelled_at = Some(1_700_000_000_000);
    sink.save(&tombstone).expect("persist cancel tombstone");

    // 2. A stale executor (missed the cancel) tries to save Resumable /
    //    InProgress / Complete progress over the tombstone — every one refused.
    for stale_status in [
        ManifestStatus::Resumable,
        ManifestStatus::InProgress,
        ManifestStatus::Complete,
    ] {
        let mut stale = InstanceManifest::fresh("cancel-guard", "software-dev-and-sales");
        stale.status = stale_status;
        assert_eq!(
            sink.save(&stale),
            Err(ManifestError::Cancelled),
            "a {stale_status:?} save must never overwrite the Cancelled tombstone"
        );
    }

    // 3. Re-cancelling (Cancelled over Cancelled) is idempotent — allowed.
    sink.save(&tombstone).expect("re-cancel is idempotent");

    // 4. The tombstone on disk is still the authoritative Cancelled state.
    let reloaded = sink.load().expect("reload tombstone");
    assert_eq!(reloaded.status, ManifestStatus::Cancelled);
    assert_eq!(reloaded.cancel_generation, 1);
    assert_eq!(reloaded.cancelled_at, Some(1_700_000_000_000));
}

// ─── #10 Supported-runtime contract (single harness + primary queue) ────────
//
// `validate_supported_contract` is the fail-closed gate the production
// instantiation command runs BEFORE reserving a slot: one supervised Symphony
// daemon resolves exactly one runner and binds exactly one primary backlog.

fn role(id: &str, harness: &str) -> RoleSpec {
    RoleSpec {
        id: id.into(),
        name: id.into(),
        harness: harness.into(),
        skills: Vec::new(),
        model: None,
        groups: Vec::new(),
    }
}

fn task_list(id: &str, group: Option<&str>) -> TaskListSpec {
    TaskListSpec {
        id: id.into(),
        name: id.into(),
        group: group.map(str::to_string),
    }
}

fn group_spec(id: &str, visibility: GroupVisibility) -> crate::company_template::spec::GroupSpec {
    crate::company_template::spec::GroupSpec {
        id: id.into(),
        name: id.into(),
        visibility,
        purpose: None,
        members: Vec::new(),
    }
}

fn contract_template(roles: Vec<RoleSpec>, task_lists: Vec<TaskListSpec>) -> CompanyTemplate {
    CompanyTemplate {
        id: "contract-probe".into(),
        name: "Contract Probe".into(),
        description: None,
        version: "1".into(),
        groups: Vec::new(),
        roles,
        stores: Vec::new(),
        task_lists,
    }
}

#[test]
fn contract_rejects_a_multi_harness_roster() {
    // codex → "codex", claude → "claude_code": two distinct presets.
    let tpl = contract_template(
        vec![role("engineer", "codex"), role("closer", "claude")],
        vec![task_list("backlog", None)],
    );
    let err = validate_supported_contract(&tpl).unwrap_err();
    assert!(
        matches!(
            err,
            ParseError::Invalid(ValidationIssue::MultipleHarnessesUnsupported { .. })
        ),
        "multi-harness roster must be rejected, got {err:?}"
    );
}

#[test]
fn contract_accepts_a_uniform_single_harness_roster() {
    // Two roles, same preset (both normalize to "pi") — accepted.
    let tpl = contract_template(
        vec![role("engineer", "buzz"), role("closer", "goose")],
        vec![task_list("backlog", None)],
    );
    validate_supported_contract(&tpl).expect("uniform roster satisfies the contract");
}

#[test]
fn contract_requires_exactly_one_primary_task_list() {
    // Zero company-shared task lists.
    let zero = contract_template(
        vec![role("engineer", "codex")],
        vec![task_list("eng-board", Some("engineering"))],
    );
    let err = validate_supported_contract(&zero).unwrap_err();
    assert!(matches!(
        err,
        ParseError::Invalid(ValidationIssue::PrimaryTaskListContract { count: 0 })
    ));

    // Two company-shared task lists — Symphony binds only one backlog.
    let two = contract_template(
        vec![role("engineer", "codex")],
        vec![task_list("backlog", None), task_list("other", None)],
    );
    let err = validate_supported_contract(&two).unwrap_err();
    assert!(matches!(
        err,
        ParseError::Invalid(ValidationIssue::PrimaryTaskListContract { count: 2 })
    ));
}

#[test]
fn contract_requires_the_primary_task_list_first() {
    // A primary exists but is not first — Symphony binds task_lists[0] as the
    // backlog, which here is group-scoped. Rejected.
    let tpl = contract_template(
        vec![role("engineer", "codex")],
        vec![
            task_list("eng-board", Some("engineering")),
            task_list("backlog", None),
        ],
    );
    let err = validate_supported_contract(&tpl).unwrap_err();
    assert!(matches!(
        err,
        ParseError::Invalid(ValidationIssue::PrimaryTaskListNotFirst)
    ));
}

#[test]
fn shipping_dev_and_sales_template_satisfies_the_runtime_contract() {
    // The builtin the production gate runs against must pass, so the cutover
    // never fail-closes spuriously against the shipping template.
    let tpl = crate::company_template::registry::get_company_template("software-dev-and-sales")
        .expect("shipping template is registered");
    validate_supported_contract(&tpl)
        .expect("software-dev-and-sales satisfies the supported-runtime contract");
}

#[test]
fn contract_rejects_a_non_public_group_before_reservation() {
    // The third leg of the supported-runtime contract: production POST /groups
    // accepts only the `public_open` preset. A `private_secure` group would fail
    // mid-provisioning (after a slot is reserved) with an opaque daemon error,
    // so validate_supported_contract rejects it at the gate — BEFORE any
    // reservation. instantiate_company_template runs this gate before reserving.
    // The error must name the offending group and its preset so the operator
    // sees the real reason, not a generic failure.
    let mut private = contract_template(
        vec![role("engineer", "codex")],
        vec![task_list("backlog", None)],
    );
    private.groups = vec![group_spec("eng", GroupVisibility::PrivateSecure)];

    let err = validate_supported_contract(&private).unwrap_err();
    assert!(
        matches!(
                   err,
                   ParseError::Invalid(ValidationIssue::UnsupportedGroupVisibility {
                       ref group,
                       ref visibility,
        }) if group == "eng" && visibility == "private_secure"
               ),
        "a private_secure group must be rejected at the contract gate, got {err:?}"
    );

    // Negative control: the same roster with a public_open group passes.
    let mut public = contract_template(
        vec![role("engineer", "codex")],
        vec![task_list("backlog", None)],
    );
    public.groups = vec![group_spec("eng", GroupVisibility::PublicOpen)];
    validate_supported_contract(&public).expect("a public_open-only roster satisfies the contract");
}

// ─── #8 Zero relay events (static source proof) ──────────────────────────────
//
// The M4 contract: the template/Symphony path never emits a Nostr relay event.
// This reads the module's own source at test time and asserts none of the
// relay-emit code paths are reachable. It adapts to files added later.

#[test]
fn company_template_module_emits_no_relay_events() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/company_template");
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read dir {:?}: {e}", dir));

    // Symbols that would indicate a relay-event emit or Nostr coupling.
    // (Comments saying "no relay events" are fine — we match code shapes.)
    let forbidden = [
        "crate::relay",
        "publish_event",
        "submit_event",
        "sign_event",
        "create_auth_event",
        "nostr::",
        "KIND_",
        "::relay::",
        "relay_publish",
    ];

    let mut offenders: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // The M4 hybrid test files themselves name the forbidden tokens in
        // this assertion (and in doc comments) — that is not production
        // coupling, so skip both the parse/plan slice and this runtime slice.
        let fname = path.file_name().and_then(|n| n.to_str());
        if matches!(
            fname,
            Some("m4_hybrid_tests.rs") | Some("m4_hybrid_runtime_tests.rs")
        ) {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {:?}: {e}", path));
        for needle in forbidden {
            // Ignore occurrences inside line comments (// ...).
            for (lineno, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if code.contains(needle) {
                    offenders.push(format!(
                        "{}:{} `{}`",
                        path.file_name().unwrap().to_string_lossy(),
                        lineno + 1,
                        needle
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "company_template reaches relay/Nostr code paths: {offenders:?}"
    );
}

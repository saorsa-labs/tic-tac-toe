//! M4 hybrid-slice tests for the `company_template` backend.
//!
//! These target the **confirmed, exported seams** of the company-template
//! parse / plan / workflow path. They assert the properties the M4 contract
//! guarantees:
//!   - TOML parse + referential validation (every `ValidationIssue` kind).
//!   - `software-dev-and-sales` → Engineering + Sales (`private_secure`) and
//!     All-Hands (`public_open`) native x0xd groups.
//!   - `plan_instantiation` is a **pure, deterministic** function of
//!     `(template, instance_id)` — byte-identical output, stable step order
//!     and sha256 step keys.
//!   - `generate_workflow_md` describes the topology + the Symphony pipeline.
//!   - **Zero relay events**: the module never reaches a Nostr relay emit.
//!
//! No production files are edited by this test module. It is wired in by the
//! module owner via `#[cfg(test)] #[path = "m4_hybrid_tests.rs"] mod m4_hybrid_tests;`
//! (the same convention as `local_stack_tests.rs`).

use super::instantiate::{
    instantiate_company, InMemorySink, JsonManifestSink, ManifestSink, ManifestStatus,
};
use super::parse::{parse_company_template, parse_lenient, validate, ParseError, ValidationIssue};
use super::plan::{instance_slug, plan_instantiation, PlanStep};
use super::provisioner::RecordingProvisioner;
use super::spec::{CompanyTemplate, GroupVisibility, StoreKind, StorePolicy};
use super::workflow::generate_workflow_md;

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

// ─── #1 Parse: the software-dev-and-sales topology ───────────────────────────

#[test]
fn parse_dev_sales_has_three_groups_with_correct_visibility() {
    let t = parsed_dev_sales();
    assert_eq!(t.id, "software-dev-and-sales");
    assert_eq!(t.name, "Software Dev & Sales");
    assert_eq!(t.groups.len(), 3);

    let engineering = t.group("engineering").expect("engineering group");
    assert_eq!(engineering.visibility, GroupVisibility::PrivateSecure);
    let sales = t.group("sales").expect("sales group");
    assert_eq!(sales.visibility, GroupVisibility::PrivateSecure);
    let all_hands = t.group("all-hands").expect("all-hands group");
    assert_eq!(all_hands.visibility, GroupVisibility::PublicOpen);
}

#[test]
fn parse_dev_sales_visibility_maps_to_native_x0xd_presets() {
    // GroupVisibility maps 1:1 to a native x0xd group preset — never a relay kind.
    assert_eq!(
        GroupVisibility::PrivateSecure.as_x0xd_preset(),
        "private_secure"
    );
    assert_eq!(GroupVisibility::PublicOpen.as_x0xd_preset(), "public_open");
}

#[test]
fn parse_dev_sales_roles_carry_harness_and_skills_only() {
    let t = parsed_dev_sales();
    let eng = t.role("staff-engineer").expect("staff-engineer role");
    assert_eq!(eng.harness, "codex");
    assert_eq!(
        eng.skills,
        vec!["code-review".to_string(), "git-ops".to_string()]
    );
    // Roles staff their declared groups (declared, not realized as x0xd members).
    assert_eq!(
        eng.groups,
        vec!["engineering".to_string(), "all-hands".to_string()]
    );
}

#[test]
fn parse_dev_sales_stores_are_typed_symphony_kinds() {
    let t = parsed_dev_sales();
    let backlog = t
        .stores
        .iter()
        .find(|s| s.id == "engineering-backlog")
        .unwrap();
    assert_eq!(backlog.kind, StoreKind::Backlog);
    assert_eq!(backlog.policy, StorePolicy::AppendOnly);
    assert_eq!(backlog.group.as_deref(), Some("engineering"));
    let proofs = t.stores.iter().find(|s| s.id == "company-proofs").unwrap();
    assert_eq!(proofs.kind, StoreKind::Proofs);
    // policy defaults to Signed, company-shared (no group).
    assert_eq!(proofs.policy, StorePolicy::Signed);
    assert!(proofs.group.is_none());
}

#[test]
fn parse_dev_sales_has_shared_task_list() {
    let t = parsed_dev_sales();
    let tl = t
        .task_lists
        .iter()
        .find(|l| l.id == "sales-pipeline")
        .unwrap();
    assert_eq!(tl.group.as_deref(), Some("sales"));
}

// ─── #2 Parse: serde defaults + snake_case wire shapes ───────────────────────

#[test]
fn parse_applies_defaults_for_omitted_fields() {
    let src = r#"
id = "minimal"
name = "Minimal"
[[groups]]
id = "g1"
name = "G1"
visibility = "public_open"
"#;
    let t = parse_company_template(src).expect("minimal parses");
    assert_eq!(t.version, "1"); // default_version
    assert!(t.description.is_none());
    assert!(t.roles.is_empty());
    assert!(t.stores.is_empty());
    assert!(t.task_lists.is_empty());
    let g = t.group("g1").unwrap();
    assert!(g.members.is_empty()); // default
    assert!(g.purpose.is_none());
}

#[test]
fn store_policy_defaults_to_signed() {
    assert_eq!(StorePolicy::default(), StorePolicy::Signed);
    assert_eq!(StorePolicy::Signed.as_x0xd_policy(), "signed");
    assert_eq!(StorePolicy::AppendOnly.as_x0xd_policy(), "append_only");
}

#[test]
fn store_kind_wire_strings_are_stable_lowercase() {
    // These strings flow into derived store topics + the generated config.
    assert_eq!(StoreKind::Backlog.as_str(), "backlog");
    assert_eq!(StoreKind::Claims.as_str(), "claims");
    assert_eq!(StoreKind::Runs.as_str(), "runs");
    assert_eq!(StoreKind::Handoffs.as_str(), "handoffs");
    assert_eq!(StoreKind::Proofs.as_str(), "proofs");
    assert_eq!(StoreKind::Generic.as_str(), "generic");
}

// ─── #3 Parse: every ValidationIssue kind ────────────────────────────────────

#[test]
fn parse_rejects_empty_template_id() {
    let src = r#"
id = "   "
name = "X"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(err, ParseError::Invalid(ValidationIssue::EmptyTemplateId));
}

#[test]
fn parse_rejects_empty_group_name() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "   "
visibility = "public_open"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(err, ParseError::Invalid(ValidationIssue::EmptyGroupName));
}

#[test]
fn parse_rejects_duplicate_group_id() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "dup"
name = "A"
visibility = "public_open"
[[groups]]
id = "dup"
name = "B"
visibility = "public_open"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::DuplicateGroupId("dup".into()))
    );
}

#[test]
fn parse_rejects_duplicate_role_id() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[roles]]
id = "r"
name = "R"
harness = "codex"
[[roles]]
id = "r"
name = "R2"
harness = "claude"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::DuplicateRoleId("r".into()))
    );
}

#[test]
fn parse_rejects_duplicate_store_id() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[stores]]
id = "s"
name = "S"
kind = "backlog"
[[stores]]
id = "s"
name = "S2"
kind = "claims"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::DuplicateStoreId("s".into()))
    );
}

#[test]
fn parse_rejects_duplicate_task_list_id() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[task_lists]]
id = "tl"
name = "TL"
[[task_lists]]
id = "tl"
name = "TL2"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::DuplicateTaskListId("tl".into()))
    );
}

#[test]
fn parse_rejects_group_member_referencing_unknown_role() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
members = ["ghost"]
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::UnknownRoleRef {
            group: "g".into(),
            role: "ghost".into(),
        })
    );
}

#[test]
fn parse_rejects_role_referencing_unknown_group() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[roles]]
id = "r"
name = "R"
harness = "codex"
groups = ["nope"]
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::UnknownGroupRef {
            field: "role `r`.groups".into(),
            group: "nope".into(),
        })
    );
}

#[test]
fn parse_rejects_store_scoped_to_unknown_group() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[stores]]
id = "s"
name = "S"
kind = "backlog"
group = "missing"
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::UnknownGroupRef {
            field: "store `s`.group".into(),
            group: "missing".into(),
        })
    );
}

#[test]
fn parse_rejects_role_with_empty_harness() {
    let src = r#"
id = "t"
name = "T"
[[groups]]
id = "g"
name = "G"
visibility = "public_open"
[[roles]]
id = "r"
name = "R"
harness = "  "
"#;
    let err = parse_company_template(src).unwrap_err();
    assert_eq!(
        err,
        ParseError::Invalid(ValidationIssue::EmptyHarness { role: "r".into() })
    );
}

#[test]
fn parse_rejects_malformed_toml_as_decode_error() {
    // Unclosed table header is unambiguously invalid TOML.
    let err = parse_company_template("[[groups\nid = \"g\"\n").unwrap_err();
    assert!(matches!(err, ParseError::Decode(_)), "got {err:?}");
}

#[test]
fn validate_accepts_a_well_formed_template_directly() {
    // `validate` is the pure, side-effect-free referential check exposed as a
    // confirmed seam. A parsed dev/sales template must pass it with no I/O.
    let t = parsed_dev_sales();
    assert!(validate(&t).is_ok());
}

// ─── #4 Parse: determinism (same input → identical result) ───────────────────

#[test]
fn parse_is_deterministic_for_success_and_error() {
    // Success path: identical bytes → identical template (fields compared via Debug).
    let a = format!("{:?}", parsed_dev_sales());
    let b = format!("{:?}", parsed_dev_sales());
    assert_eq!(a, b);

    // Error path: identical bad input → identical error variant + message.
    let bad = "\nid = \"   \"\nname = \"X\"\n".to_string();
    let e1 = format!("{}", parse_company_template(&bad).unwrap_err());
    let e2 = format!("{}", parse_company_template(&bad).unwrap_err());
    assert_eq!(e1, e2);
}

// ─── #5 Plan: pure + deterministic ───────────────────────────────────────────

#[test]
fn plan_is_pure_deterministic_byte_identical() {
    let t = parsed_dev_sales();
    let p1 = plan_instantiation(&t, "instance-001");
    let p2 = plan_instantiation(&t, "instance-001");

    // Every observable field is byte-identical for identical inputs.
    assert_eq!(p1.instance_id, p2.instance_id);
    assert_eq!(p1.template_id, p2.template_id);
    assert_eq!(format!("{:?}", p1.steps), format!("{:?}", p2.steps));
    assert_eq!(p1.symphony_config_toml, p2.symphony_config_toml);
    assert_eq!(p1.workflow_md, p2.workflow_md);
    assert_eq!(p1.step_keys(), p2.step_keys());
}

#[test]
fn plan_step_order_is_groups_then_stores_then_tasklists_then_config_then_workflow() {
    let t = parsed_dev_sales();
    let plan = plan_instantiation(&t, "ord");

    let kinds: Vec<&'static str> = plan.steps.iter().map(|s| s.kind_label()).collect();
    // 3 groups, 2 stores, 1 task list, 1 config, 1 workflow = 8 steps, in that order.
    assert_eq!(
        kinds,
        vec![
            "group",
            "group",
            "group",
            "store",
            "store",
            "task_list",
            "symphony_config",
            "workflow_md",
        ]
    );
    assert_eq!(plan.step_count(), 8);
}

#[test]
fn plan_keys_are_deterministic_and_distinct_per_instance() {
    let t = parsed_dev_sales();
    let p_a = plan_instantiation(&t, "instance-A");
    let p_b = plan_instantiation(&t, "instance-B");

    // Same instance reproduces the same keys (idempotency substrate).
    assert_eq!(
        p_a.step_keys(),
        plan_instantiation(&t, "instance-A").step_keys()
    );
    // Different instance → every key differs (keys embed instance_id).
    assert_ne!(p_a.step_keys(), p_b.step_keys());
    // Keys are 64-hex-char sha256 digests.
    for k in p_a.step_keys() {
        assert_eq!(k.len(), 64, "key not sha256 hex: {k}");
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()), "non-hex key: {k}");
    }
}

#[test]
fn plan_store_topics_are_company_scoped_and_instance_namespaced() {
    let t = parsed_dev_sales();
    let plan = plan_instantiation(&t, "Acme Co. 2026!");

    // instance_slug lowercases + collapses non-topic-safe chars to '-'.
    let store_step = plan
        .steps
        .iter()
        .find_map(|s| match s {
            PlanStep::CreateStore {
                local_id, topic, ..
            } if local_id == "company-proofs" => Some(topic.clone()),
            _ => None,
        })
        .expect("company-proofs store step");
    // Company-scoped: no x0x.group.<gid> prefix (no group-local membership / crypto).
    assert!(
        !store_step.contains("x0x.group"),
        "topic must be company-scoped: {store_step}"
    );
    assert!(
        store_step.starts_with("ttt.acme-co-2026.store.proofs.company-proofs"),
        "topic must be instance-namespaced: {store_step}"
    );
}

#[test]
fn instance_slug_is_topic_safe_bounded_and_defaultable() {
    assert_eq!(instance_slug("Acme Co. 2026!"), "acme-co-2026");
    assert_eq!(instance_slug("---"), "default"); // collapses to empty → default
    assert_eq!(instance_slug(""), "default");
    // Bounded to 48 chars so derived topics stay reasonable.
    let long = "a".repeat(200);
    assert_eq!(instance_slug(&long).len(), 48);
}

// ─── #6 Workflow.md generation ───────────────────────────────────────────────

#[test]
fn workflow_md_documents_topology_and_symphony_pipeline() {
    let t = parsed_dev_sales();
    let plan = plan_instantiation(&t, "wf-instance");
    let md = generate_workflow_md(&t, &plan);

    assert!(md.starts_with("# WORKFLOW.md"));
    // All three groups render with their visibility.
    assert!(md.contains("Engineering"));
    assert!(md.contains("Sales"));
    assert!(md.contains("All-Hands"));
    assert!(md.contains("private_secure"));
    assert!(md.contains("public_open"));
    // The five-stage Symphony pipeline is described in order.
    assert!(md.contains("Backlog"));
    assert!(md.contains("Claims"));
    assert!(md.contains("Runs"));
    assert!(md.contains("Handoffs"));
    assert!(md.contains("Proofs"));
}

#[test]
fn workflow_md_is_deterministic() {
    let t = parsed_dev_sales();
    let plan = plan_instantiation(&t, "det");
    let a = generate_workflow_md(&t, &plan);
    let b = generate_workflow_md(&t, &plan_instantiation(&t, "det"));
    assert_eq!(a, b);
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
    assert!(manifest.is_complete());
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
        // This very test file mentions the forbidden tokens in assertions, so
        // skip it to avoid a self-match.
        if path.file_name().and_then(|n| n.to_str()) == Some("m4_hybrid_tests.rs") {
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

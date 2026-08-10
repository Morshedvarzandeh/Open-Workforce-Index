//! Persistence boundaries for the public index and the private local allocator.
//!
//! The two store types deliberately have unrelated read/write traits and reject
//! opening a database initialized for the other trust domain. Public export
//! functions accept only [`PublicIndexRead`], so private ledger values cannot
//! accidentally enter a public snapshot through this API.

mod convert;
mod error;
mod export;
mod private;
mod public;
mod records;
mod schema;
mod traits;
mod validate;

pub use crate::{
    error::StoreError,
    export::{PublicIndexExport, build_public_export},
    private::PrivateLocalStore,
    public::{PublicIndexStore, ReadOnlyPublicIndexStore},
    records::{
        CandidateQuoteAuditRecord, ModelReleaseRecord, PrivateOutcomeRecord,
        ProviderOfferingRecord, PublicEvidenceRecord, QuoteRecord, RejectedCandidateAuditRecord,
        SelectionExplanationAuditRecord, SnapshotRecord, WorkerProfileRecord,
    },
    traits::{PrivateLedgerRead, PrivateLedgerWrite, PublicIndexRead, PublicIndexWrite},
    validate::worker_configuration_sha256,
};

// The test module below lives at the crate root and reaches for internals by
// `use super::*`. Re-exporting them here keeps that block untouched by the
// split: the tests are the only safety net this crate has, so they were moved
// nowhere and not a line of them was edited.
#[allow(unused_imports)]
pub(crate) use crate::{
    convert::{public_evidence_row, quote_row, snapshot_row, worker_profile_row},
    schema::{IDENTITY_SCHEMA, PRIVATE_SCHEMA, PUBLIC_SCHEMA},
    validate::{
        configure_file_connection, configure_memory_connection, decode_bool, decode_evidence_tier,
        decode_privacy_class, decode_validation_kind, decode_verification_policy,
        encode_evidence_tier, encode_privacy_class, encode_validation_kind,
        encode_verification_policy, from_i64, hash_component, hash_id_list,
        initialize_or_validate_store, lower_hex, prepare_private_database_file,
        read_provider_offering, required_snapshot_member, schema_version,
        secure_private_sqlite_files, to_i64, validate_canonical_identifier,
        validate_export_dependency_closure, validate_finite, validate_identity,
        validate_manifest_list, validate_outcome_quote_link, validate_probability,
        validate_quote_audit, validate_required_text, validate_schema_version, validate_sha256,
        validate_snapshot_dependencies, worker_identity,
    },
};
#[allow(unused_imports)]
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};

#[allow(unused_imports)]
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
#[allow(unused_imports)]
use workforce_domain::{
    BenchmarkId, DecisionId, EvidenceTier, ModelReleaseId, OfferingId, OutcomeEvent, PrivacyClass,
    SkillId, TaskId, ValidationKind, VerificationPolicy, WorkerId, WorkerIdentity,
};

pub(crate) const PUBLIC_STORE_KIND: &str = "public_index";
pub(crate) const PRIVATE_STORE_KIND: &str = "private_local";
pub(crate) const PUBLIC_SCHEMA_VERSION: i64 = 2;
pub(crate) const PRIVATE_SCHEMA_VERSION: i64 = 2;

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, time::SystemTime};

    use serde_json::Value;

    use super::*;

    const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIGEST_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn public_records_round_trip_through_export_boundary() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let model = sample_model();
        let offering = sample_offering();
        let worker = sample_worker();
        let evidence = sample_evidence();
        let snapshot = sample_snapshot();

        store.append_model_release(&model).expect("append model");
        store
            .append_provider_offering(&offering)
            .expect("append offering");
        store.append_worker_profile(&worker).expect("append worker");
        store.append_evidence(&evidence).expect("append evidence");
        store.append_snapshot(&snapshot).expect("append snapshot");

        let export = build_public_export(&store, &snapshot.id).expect("public export");
        assert_eq!(export.model_releases, vec![model]);
        assert_eq!(export.provider_offerings, vec![offering]);
        assert_eq!(export.worker_profiles, vec![worker]);
        assert_eq!(export.evidence, vec![evidence]);
        assert_eq!(export.snapshot, snapshot.clone());
        assert_eq!(
            store.snapshot(&snapshot.id).expect("snapshot"),
            Some(snapshot)
        );
        assert_eq!(store.snapshot("missing").expect("missing snapshot"), None);
    }

    #[test]
    fn snapshot_export_is_not_changed_by_later_appends() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_snapshot_chain(&store);

        let mut model = sample_model();
        model.id = ModelReleaseId("model:later".to_owned());
        model.artifact_sha256 = DIGEST_C.to_owned();
        store.append_model_release(&model).expect("later model");

        let mut offering = sample_offering();
        offering.id = OfferingId("offering:later".to_owned());
        offering.model_release_id = model.id;
        store
            .append_provider_offering(&offering)
            .expect("later offering");

        let mut worker = sample_worker();
        worker.id = WorkerId("worker:later".to_owned());
        worker.offering_id = offering.id.clone();
        worker.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &worker,
            offering.model_release_id,
            offering.provider,
        ));
        store.append_worker_profile(&worker).expect("later worker");

        let export = build_public_export(&store, "snapshot:test").expect("old export");
        assert_eq!(export.model_releases.len(), 1);
        assert_eq!(export.provider_offerings.len(), 1);
        assert_eq!(export.worker_profiles.len(), 1);
        assert_eq!(export.evidence.len(), 1);
        assert_eq!(export.model_releases[0].id.0, "model:test");
    }

    #[test]
    fn tampered_snapshot_digest_count_and_members_are_rejected() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        store
            .append_evidence(&sample_evidence())
            .expect("append evidence");

        let mut digest = sample_snapshot();
        digest.content_sha256 = DIGEST_C.to_owned();
        assert!(matches!(
            store.append_snapshot(&digest),
            Err(StoreError::SnapshotDigestMismatch { .. })
        ));

        let mut count = sample_snapshot();
        count.evidence_count += 1;
        assert!(matches!(
            store.append_snapshot(&count),
            Err(StoreError::InvalidSnapshotManifest { .. })
        ));

        let mut member = sample_snapshot();
        member.id = "snapshot:bad-member".to_owned();
        member.evidence_ids[0] = "evidence:missing".to_owned();
        member.content_sha256 = member
            .calculate_content_sha256()
            .expect("recalculate tampered manifest");
        assert!(matches!(
            store.append_snapshot(&member),
            Err(StoreError::SnapshotMemberMissing { .. })
        ));

        let mut closure = sample_snapshot();
        closure.id = "snapshot:open-dependency".to_owned();
        closure.model_release_ids.clear();
        closure.model_release_count = 0;
        closure.content_sha256 = closure
            .calculate_content_sha256()
            .expect("recalculate open manifest");
        assert!(matches!(
            store.append_snapshot(&closure),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));
    }

    #[test]
    fn snapshot_requires_the_complete_offering_revision_chain() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let offerings = append_offering_revision_chain(&store);

        let missing_immediate_predecessor = SnapshotRecord::new(
            "snapshot:missing-immediate-predecessor",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-1",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        assert!(matches!(
            store.append_snapshot(&missing_immediate_predecessor),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));

        let missing_transitive_predecessor = SnapshotRecord::new(
            "snapshot:missing-transitive-predecessor",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-2",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[1].id.clone(), offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        assert!(matches!(
            store.append_snapshot(&missing_transitive_predecessor),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));

        let complete = SnapshotRecord::new(
            "snapshot:complete-revision-chain",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-3",
            vec![ModelReleaseId("model:test".to_owned())],
            offerings
                .iter()
                .map(|offering| offering.id.clone())
                .collect(),
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        store.append_snapshot(&complete).expect("complete snapshot");

        let export = build_public_export(&store, &complete.id).expect("complete export");
        assert_eq!(export.provider_offerings.len(), 3);
    }

    #[test]
    fn export_revalidates_offering_revision_closure() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let offerings = append_offering_revision_chain(&store);
        let incomplete = SnapshotRecord::new(
            "snapshot:unchecked-incomplete-chain",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "revision-chain-unchecked",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![offerings[1].id.clone(), offerings[2].id.clone()],
            vec![],
            vec![],
        )
        .expect("canonical snapshot");
        insert_snapshot_without_validation(&store, &incomplete);

        assert!(matches!(
            build_public_export(&store, &incomplete.id),
            Err(StoreError::SnapshotDependencyNotClosed(_))
        ));
    }

    #[test]
    fn release_only_evidence_and_unknown_sample_count_round_trip() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let mut evidence = sample_evidence();
        evidence.worker_id = None;
        evidence.sample_count = None;
        store.append_evidence(&evidence).expect("release evidence");
        let snapshot = SnapshotRecord::new(
            "snapshot:release-only",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "abc124",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![],
            vec![],
            vec![evidence.id.clone()],
        )
        .expect("release-only snapshot");
        store.append_snapshot(&snapshot).expect("snapshot");

        let export = build_public_export(&store, &snapshot.id).expect("export");
        assert_eq!(export.evidence, vec![evidence]);
    }

    #[test]
    fn worker_configuration_digest_is_unique_and_capabilities_round_trip() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        store
            .append_provider_offering(&sample_offering())
            .expect("append offering");
        let worker = sample_worker();
        store.append_worker_profile(&worker).expect("append worker");
        assert_eq!(
            store.worker_profiles().expect("workers"),
            vec![worker.clone()]
        );

        let mut duplicate_configuration = worker;
        duplicate_configuration.id = WorkerId("worker:duplicate".to_owned());
        assert!(
            store
                .append_worker_profile(&duplicate_configuration)
                .is_err()
        );
    }

    #[test]
    fn worker_configuration_digest_is_recomputed_from_the_stored_offering() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let model = sample_model();
        let offering = sample_offering();
        store.append_model_release(&model).expect("append model");
        store
            .append_provider_offering(&offering)
            .expect("append offering");

        let mut tampered_configuration = sample_worker();
        tampered_configuration.harness_version = "tampered".to_owned();
        assert!(matches!(
            store.append_worker_profile(&tampered_configuration),
            Err(StoreError::WorkerConfigurationDigestMismatch { .. })
        ));

        let mut provider_relabel = sample_worker();
        provider_relabel.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &provider_relabel,
            model.id,
            "relabeled-provider".to_owned(),
        ));
        assert!(matches!(
            store.append_worker_profile(&provider_relabel),
            Err(StoreError::WorkerConfigurationDigestMismatch { .. })
        ));

        let worker = sample_worker();
        store
            .append_worker_profile(&worker)
            .expect("authoritative provider digest");
    }

    #[test]
    fn public_entity_ids_and_provider_are_canonical_before_insert() {
        let store = PublicIndexStore::in_memory().expect("public store");

        let mut model = sample_model();
        model.id = ModelReleaseId(" model:test".to_owned());
        assert!(matches!(
            store.append_model_release(&model),
            Err(StoreError::NonCanonicalIdentifier {
                field: "model_release.id"
            })
        ));
        store
            .append_model_release(&sample_model())
            .expect("append canonical model");

        let mut offering = sample_offering();
        offering.id = OfferingId(" offering:test".to_owned());
        assert!(matches!(
            store.append_provider_offering(&offering),
            Err(StoreError::NonCanonicalIdentifier {
                field: "provider_offering.id"
            })
        ));

        let mut offering = sample_offering();
        offering.provider = "example ".to_owned();
        assert!(matches!(
            store.append_provider_offering(&offering),
            Err(StoreError::NonCanonicalIdentifier {
                field: "provider_offering.provider"
            })
        ));
        store
            .append_provider_offering(&sample_offering())
            .expect("append canonical offering");

        let mut worker = sample_worker();
        worker.id = WorkerId("worker:test ".to_owned());
        assert!(matches!(
            store.append_worker_profile(&worker),
            Err(StoreError::NonCanonicalIdentifier {
                field: "worker_profile.id"
            })
        ));
    }

    #[test]
    fn evidence_required_identity_and_text_fields_are_validated_before_insert() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);

        let mut evidence = sample_evidence();
        evidence.id = " evidence:test".to_owned();
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.model_release_id = ModelReleaseId(" ".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.model_release_id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.skill_id = SkillId("skill:rust ".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.skill_id"
            })
        ));

        let mut evidence = sample_evidence();
        evidence.benchmark_id = BenchmarkId("".to_owned());
        assert!(matches!(
            store.append_evidence(&evidence),
            Err(StoreError::NonCanonicalIdentifier {
                field: "evidence.benchmark_id"
            })
        ));

        for field in [
            "evidence.metric",
            "evidence.unit",
            "evidence.adapter_version",
            "evidence.source_url",
            "evidence.license",
        ] {
            let mut evidence = sample_evidence();
            match field {
                "evidence.metric" => evidence.metric = " ".to_owned(),
                "evidence.unit" => evidence.unit = " ".to_owned(),
                "evidence.adapter_version" => evidence.adapter_version = " ".to_owned(),
                "evidence.source_url" => evidence.source_url = " ".to_owned(),
                "evidence.license" => evidence.license = " ".to_owned(),
                _ => unreachable!("test field is exhaustive"),
            }
            assert!(matches!(
                store.append_evidence(&evidence),
                Err(StoreError::EmptyRequiredField { field: actual }) if actual == field
            ));
        }
    }

    #[test]
    fn sha256_fields_reject_non_hex_and_uppercase_values() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let mut model = sample_model();
        model.artifact_sha256 = "G".repeat(64);
        assert!(matches!(
            store.append_model_release(&model),
            Err(StoreError::InvalidSha256 { .. })
        ));

        let mut snapshot = sample_snapshot();
        snapshot.content_sha256 = "z".repeat(64);
        assert!(matches!(
            snapshot.validate(),
            Err(StoreError::InvalidSha256 { .. })
        ));
    }

    #[test]
    fn offering_revisions_preserve_identity_and_current_query_excludes_predecessor() {
        let store = PublicIndexStore::in_memory().expect("public store");
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let original = sample_offering();
        store
            .append_provider_offering(&original)
            .expect("original offering");

        let mut revision = original.clone();
        revision.id = OfferingId("offering:revision".to_owned());
        revision.supersedes_offering_id = Some(original.id.clone());
        revision.effective_from_epoch_ms += 1_000;
        revision.fixed_request_micros = 50;
        store
            .append_provider_offering(&revision)
            .expect("valid revision");

        assert_eq!(
            store
                .current_provider_offerings(original.effective_from_epoch_ms)
                .expect("current before revision"),
            vec![original.clone()]
        );
        assert_eq!(
            store
                .current_provider_offerings(revision.effective_from_epoch_ms)
                .expect("current after revision"),
            vec![revision]
        );

        let mut invalid = original;
        invalid.id = OfferingId("offering:invalid".to_owned());
        invalid.supersedes_offering_id = Some(OfferingId("offering:revision".to_owned()));
        invalid.provider = "different-provider".to_owned();
        invalid.effective_from_epoch_ms += 2_000;
        assert!(store.append_provider_offering(&invalid).is_err());

        let mut overlap_base = sample_offering();
        overlap_base.id = OfferingId("offering:overlap-base".to_owned());
        overlap_base.effective_until_epoch_ms = Some(overlap_base.effective_from_epoch_ms + 5_000);
        store
            .append_provider_offering(&overlap_base)
            .expect("explicit interval base");
        let mut overlap_revision = overlap_base.clone();
        overlap_revision.id = OfferingId("offering:overlap-revision".to_owned());
        overlap_revision.supersedes_offering_id = Some(overlap_base.id);
        overlap_revision.effective_from_epoch_ms += 1_000;
        overlap_revision.effective_until_epoch_ms = None;
        assert!(
            store.append_provider_offering(&overlap_revision).is_err(),
            "an explicit predecessor interval cannot overlap its successor"
        );
    }

    #[test]
    fn public_evidence_requires_an_existing_model_release() {
        let store = PublicIndexStore::in_memory().expect("public store");
        let error = store
            .append_evidence(&sample_evidence())
            .expect_err("foreign key must reject orphan evidence");
        assert!(matches!(error, StoreError::Sqlite(_)));
    }

    #[test]
    fn worker_specific_evidence_must_match_its_release() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        let mut other_model = sample_model();
        other_model.id = ModelReleaseId("model:other".to_owned());
        store
            .append_model_release(&other_model)
            .expect("other release");

        let mut mismatched = sample_evidence();
        mismatched.model_release_id = other_model.id;
        assert!(store.append_evidence(&mismatched).is_err());

        mismatched.id = "evidence:release-only".to_owned();
        mismatched.worker_id = None;
        store
            .append_evidence(&mismatched)
            .expect("release-only evidence is valid");
    }

    #[test]
    fn public_records_cannot_be_updated_or_deleted() {
        let store = PublicIndexStore::in_memory().expect("public store");
        append_public_identity_chain(&store);
        assert!(
            store
                .connection
                .execute(
                    "UPDATE model_releases SET developer = 'changed' WHERE id = ?1",
                    ["model:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute("DELETE FROM model_releases WHERE id = ?1", ["model:test"])
                .is_err()
        );
        assert!(
            store
                .connection
                .execute(
                    "UPDATE provider_offerings SET fixed_request_micros = 1 WHERE id = ?1",
                    ["offering:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute("DELETE FROM worker_profiles WHERE id = ?1", ["worker:test"])
                .is_err()
        );
    }

    #[test]
    fn read_only_public_handle_can_export_existing_index() {
        let path = temporary_database_path("read-only");
        {
            let store = PublicIndexStore::open(&path).expect("public file store");
            append_public_snapshot_chain(&store);
        }
        {
            let store = ReadOnlyPublicIndexStore::open(&path).expect("read-only store");
            let export = build_public_export(&store, "snapshot:test").expect("read-only export");
            assert_eq!(export.model_releases.len(), 1);
            assert_eq!(export.provider_offerings.len(), 1);
            assert_eq!(export.worker_profiles.len(), 1);
        }
        remove_sqlite_files(&path);
    }

    #[test]
    fn private_quote_and_outcome_round_trip() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let quote = sample_quote();
        let outcome = sample_outcome(Some(quote.decision_id.clone()));

        store.append_quote(&quote).expect("append quote");
        store.append_outcome(&outcome).expect("append outcome");

        assert_eq!(store.quotes().expect("quotes"), vec![quote]);
        assert_eq!(store.outcomes().expect("outcomes"), vec![outcome]);
    }

    #[test]
    fn incomplete_quote_audits_are_rejected() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut missing_selected_candidate = sample_quote();
        missing_selected_candidate.eligible_candidates.clear();
        missing_selected_candidate.pareto_worker_ids.clear();
        assert!(matches!(
            store.append_quote(&missing_selected_candidate),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut missing_checker = sample_quote();
        missing_checker.verification_policy = VerificationPolicy::MakerChecker;
        assert!(matches!(
            store.append_quote(&missing_checker),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut malformed_reason = sample_quote();
        malformed_reason.rejected_candidates[0].reasons = vec![serde_json::json!("opaque")];
        assert!(matches!(
            store.append_quote(&malformed_reason),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut wrong_rank = sample_quote();
        wrong_rank.eligible_candidates[0].rank = 2;
        assert!(matches!(
            store.append_quote(&wrong_rank),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut conflicting_cost = sample_quote();
        conflicting_cost.eligible_candidates[0].cost_breakdown["expected_cash_micros"] =
            serde_json::json!(1);
        assert!(matches!(
            store.append_quote(&conflicting_cost),
            Err(StoreError::InvalidQuoteAudit(_))
        ));

        let mut empty_failed_decision = sample_quote();
        empty_failed_decision.selected_worker_id = None;
        empty_failed_decision.selected_checker_worker_id = None;
        empty_failed_decision.expected_cash_micros = None;
        empty_failed_decision.expected_quota_milliunits = None;
        empty_failed_decision.expected_success_probability = None;
        empty_failed_decision.p95_latency_ms = None;
        empty_failed_decision.eligible_candidates.clear();
        empty_failed_decision.rejected_candidates.clear();
        empty_failed_decision.pareto_worker_ids.clear();
        empty_failed_decision.selection_explanation = None;
        assert!(matches!(
            store.append_quote(&empty_failed_decision),
            Err(StoreError::InvalidQuoteAudit(_))
        ));
    }

    #[test]
    fn linked_outcome_maker_must_match_the_quote() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let quote = sample_quote();
        store.append_quote(&quote).expect("append quote");

        let mut outcome = sample_outcome(Some(quote.decision_id));
        outcome.event.worker_id = WorkerId("worker:not-selected".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));
    }

    #[test]
    fn maker_checker_outcome_requires_the_selected_checker() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let checker_id = WorkerId("worker:checker".to_owned());
        let mut quote = sample_quote();
        quote.verification_policy = VerificationPolicy::MakerChecker;
        quote.selected_checker_worker_id = Some(checker_id.clone());
        quote.eligible_candidates[0].checker_worker_id = Some(checker_id.clone());
        store.append_quote(&quote).expect("append quote");

        let mut outcome = sample_outcome(Some(quote.decision_id.clone()));
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));

        outcome.checker_worker_id = Some(WorkerId("worker:wrong-checker".to_owned()));
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::InvalidOutcomeLink(_))
        ));

        outcome.checker_worker_id = Some(checker_id);
        outcome.event.validation_kind = ValidationKind::IndependentModel;
        store
            .append_outcome(&outcome)
            .expect("selected checker outcome");
    }

    #[test]
    fn non_checker_policies_reject_unselected_model_checkers() {
        for policy in [
            VerificationPolicy::Deterministic,
            VerificationPolicy::HumanApproval,
        ] {
            let store = PrivateLocalStore::in_memory().expect("private store");
            let mut quote = sample_quote();
            quote.verification_policy = policy;
            store.append_quote(&quote).expect("append quote");

            let mut outcome = sample_outcome(Some(quote.decision_id));
            outcome.checker_worker_id = Some(WorkerId("worker:unselected-checker".to_owned()));
            assert!(matches!(
                store.append_outcome(&outcome),
                Err(StoreError::InvalidOutcomeLink(_))
            ));
        }
    }

    #[test]
    fn all_rejected_routing_decision_round_trips_without_winner_fields() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut quote = sample_quote();
        quote.selected_worker_id = None;
        quote.selected_checker_worker_id = None;
        quote.verification_policy = VerificationPolicy::MakerChecker;
        quote.expected_cash_micros = None;
        quote.expected_quota_milliunits = None;
        quote.expected_success_probability = None;
        quote.p95_latency_ms = None;
        quote.eligible_candidates.clear();
        quote.pareto_worker_ids.clear();
        quote.selection_explanation = None;

        store.append_quote(&quote).expect("failed routing decision");
        assert_eq!(store.quotes().expect("quotes"), vec![quote]);
        assert!(
            store
                .append_outcome(&sample_outcome(Some(DecisionId(
                    "decision:test".to_owned()
                ))))
                .is_err(),
            "an outcome cannot attach to a quote without a selected worker"
        );
    }

    #[test]
    fn private_outcome_quote_link_is_enforced() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let outcome = sample_outcome(Some(DecisionId("missing".to_owned())));
        let error = store
            .append_outcome(&outcome)
            .expect_err("an unknown quote must be rejected");
        assert!(matches!(error, StoreError::InvalidOutcomeLink(_)));
    }

    #[test]
    fn maker_cannot_be_its_own_checker() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.checker_worker_id = Some(outcome.event.worker_id.clone());
        assert!(store.append_outcome(&outcome).is_err());
    }

    #[test]
    fn private_outcome_identity_fields_are_canonical_before_insert() {
        let store = PrivateLocalStore::in_memory().expect("private store");

        let mut outcome = sample_outcome(None);
        outcome.event.task_id = TaskId(" ".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.task_id"
            })
        ));

        let mut outcome = sample_outcome(None);
        outcome.event.worker_id = WorkerId(" worker:test".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.worker_id"
            })
        ));

        let mut outcome = sample_outcome(None);
        outcome.event.skill_id = SkillId("skill:rust ".to_owned());
        assert!(matches!(
            store.append_outcome(&outcome),
            Err(StoreError::NonCanonicalIdentifier {
                field: "outcome.skill_id"
            })
        ));
    }

    #[test]
    fn private_records_cannot_be_updated_or_deleted() {
        let store = PrivateLocalStore::in_memory().expect("private store");
        store.append_quote(&sample_quote()).expect("append quote");
        assert!(
            store
                .connection
                .execute(
                    "UPDATE routing_quotes SET expected_cash_micros = 0 WHERE decision_id = ?1",
                    ["decision:test"],
                )
                .is_err()
        );
        assert!(
            store
                .connection
                .execute(
                    "DELETE FROM routing_quotes WHERE decision_id = ?1",
                    ["decision:test"],
                )
                .is_err()
        );
    }

    #[test]
    fn public_and_private_schemas_are_physically_separate() {
        let public = PublicIndexStore::in_memory().expect("public store");
        let private = PrivateLocalStore::in_memory().expect("private store");

        assert!(table_exists(&public.connection, "model_releases"));
        assert!(!table_exists(&public.connection, "routing_quotes"));
        assert!(table_exists(&private.connection, "routing_quotes"));
        assert!(!table_exists(&private.connection, "model_releases"));
    }

    #[test]
    fn public_export_cannot_contain_private_outcome_markers() {
        const REPOSITORY_MARKER: &str = "private-repository-marker";
        const METADATA_MARKER: &str = "private-metadata-marker";

        let public = PublicIndexStore::in_memory().expect("public store");
        append_public_snapshot_chain(&public);

        let private = PrivateLocalStore::in_memory().expect("private store");
        let mut outcome = sample_outcome(None);
        outcome.event.repository_scope = Some(REPOSITORY_MARKER.to_owned());
        outcome.event.metadata = Value::String(METADATA_MARKER.to_owned());
        private
            .append_outcome(&outcome)
            .expect("append private outcome");

        let private_json = serde_json::to_string(&private.outcomes().expect("private outcomes"))
            .expect("serialize private outcomes");
        assert!(private_json.contains(REPOSITORY_MARKER));
        assert!(private_json.contains(METADATA_MARKER));

        let public_json =
            serde_json::to_string(&build_public_export(&public, "snapshot:test").expect("export"))
                .expect("serialize export");
        assert!(!public_json.contains(REPOSITORY_MARKER));
        assert!(!public_json.contains(METADATA_MARKER));
    }

    #[test]
    fn a_file_cannot_change_trust_domains() {
        let path = temporary_database_path("identity");
        {
            let public = PublicIndexStore::open(&path).expect("public file store");
            drop(public);
        }

        let error = match PrivateLocalStore::open(&path) {
            Ok(_) => panic!("private store must reject public database"),
            Err(error) => error,
        };
        assert!(matches!(error, StoreError::StoreKindMismatch { .. }));
        remove_sqlite_files(&path);
    }

    #[test]
    fn unknown_schema_versions_are_rejected_before_use() {
        let path = temporary_database_path("future-schema");
        {
            let connection = Connection::open(&path).expect("database");
            connection
                .pragma_update(None, "user_version", 99)
                .expect("future version");
        }
        let error = match PublicIndexStore::open(&path) {
            Ok(_) => panic!("future schema must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::UnsupportedSchemaVersion {
                expected: PUBLIC_SCHEMA_VERSION,
                actual: 99
            }
        ));
        remove_sqlite_files(&path);
    }

    #[test]
    fn failed_first_time_schema_initialization_rolls_back_completely() {
        let connection = Connection::open_in_memory().expect("database");
        configure_memory_connection(&connection).expect("configure");
        let failing_schema = "
            CREATE TABLE partial_initialization (id INTEGER PRIMARY KEY) STRICT;
            THIS IS DELIBERATELY INVALID SQL;
            PRAGMA user_version = 2;
        ";
        assert!(
            initialize_or_validate_store(
                &connection,
                PUBLIC_STORE_KIND,
                PUBLIC_SCHEMA_VERSION,
                failing_schema,
            )
            .is_err()
        );
        let application_tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_schema
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .expect("table count");
        assert_eq!(application_tables, 0);
        assert_eq!(schema_version(&connection).expect("schema version"), 0);

        initialize_or_validate_store(
            &connection,
            PUBLIC_STORE_KIND,
            PUBLIC_SCHEMA_VERSION,
            PUBLIC_SCHEMA,
        )
        .expect("clean retry");
    }

    #[cfg(unix)]
    #[test]
    fn private_database_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_database_path("permissions");
        {
            let store = PrivateLocalStore::open(&path).expect("private file store");
            store.append_quote(&sample_quote()).expect("append quote");
            let mode = fs::metadata(&path)
                .expect("database metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        remove_sqlite_files(&path);
    }

    fn table_exists(connection: &Connection, table: &str) -> bool {
        connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |_| Ok(()),
            )
            .optional()
            .expect("schema query")
            .is_some()
    }

    fn sample_model() -> ModelReleaseRecord {
        ModelReleaseRecord {
            id: ModelReleaseId("model:test".to_owned()),
            developer: "example-developer".to_owned(),
            model_family: "example-family".to_owned(),
            released_at: "2026-08-01T00:00:00Z".to_owned(),
            context_window_tokens: 128_000,
            source_url: "https://example.test/model".to_owned(),
            artifact_sha256: DIGEST_A.to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_evidence() -> PublicEvidenceRecord {
        PublicEvidenceRecord {
            id: "evidence:test".to_owned(),
            model_release_id: ModelReleaseId("model:test".to_owned()),
            worker_id: Some(WorkerId("worker:test".to_owned())),
            skill_id: SkillId("skill:rust".to_owned()),
            benchmark_id: BenchmarkId("benchmark:test".to_owned()),
            evidence_tier: EvidenceTier::CommunityReproducible,
            raw_score: 82.0,
            metric: "pass_rate".to_owned(),
            unit: "percent".to_owned(),
            normalized_score: Some(0.82),
            adapter_version: "example-adapter@1".to_owned(),
            sample_count: Some(50),
            observed_at: "2026-08-02T00:00:00Z".to_owned(),
            source_url: "https://example.test/evidence".to_owned(),
            artifact_sha256: DIGEST_B.to_owned(),
            license: "Apache-2.0".to_owned(),
        }
    }

    fn sample_offering() -> ProviderOfferingRecord {
        ProviderOfferingRecord {
            id: OfferingId("offering:test".to_owned()),
            model_release_id: ModelReleaseId("model:test".to_owned()),
            provider: "example".to_owned(),
            supersedes_offering_id: None,
            effective_from_epoch_ms: 1_754_006_400_000,
            effective_until_epoch_ms: None,
            currency: "USD".to_owned(),
            input_micros_per_million_tokens: 1_000_000,
            output_micros_per_million_tokens: 3_000_000,
            fixed_request_micros: 0,
            quota_milliunits_per_request: 1_000,
            context_window_tokens: 128_000,
            source_url: "https://example.test/pricing".to_owned(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        }
    }

    fn sample_worker() -> WorkerProfileRecord {
        let offering = sample_offering();
        let mut worker = WorkerProfileRecord {
            id: WorkerId("worker:test".to_owned()),
            offering_id: offering.id.clone(),
            harness_id: "raw-api".to_owned(),
            harness_version: "1".to_owned(),
            reasoning_configuration: "standard".to_owned(),
            system_prompt_sha256: DIGEST_A.to_owned(),
            skill_pack_version: "rust@1".to_owned(),
            toolset_version: "tools@1".to_owned(),
            execution_policy_sha256: DIGEST_A.to_owned(),
            supported_skill_ids: BTreeSet::from([SkillId("skill:rust".to_owned())]),
            tools: BTreeSet::from(["shell".to_owned()]),
            privacy_clearance: PrivacyClass::ConfidentialContent,
            configuration_sha256: String::new(),
            recorded_at: "2026-08-02T00:00:00Z".to_owned(),
        };
        worker.configuration_sha256 = worker_configuration_sha256(&worker_identity(
            &worker,
            offering.model_release_id,
            offering.provider,
        ));
        worker
    }

    fn sample_snapshot() -> SnapshotRecord {
        SnapshotRecord::new(
            "snapshot:test",
            "2026-08-03T00:00:00Z",
            "0.1.0",
            "abc123",
            vec![ModelReleaseId("model:test".to_owned())],
            vec![OfferingId("offering:test".to_owned())],
            vec![WorkerId("worker:test".to_owned())],
            vec!["evidence:test".to_owned()],
        )
        .expect("canonical snapshot")
    }

    fn append_public_identity_chain(store: &PublicIndexStore) {
        store
            .append_model_release(&sample_model())
            .expect("append model");
        store
            .append_provider_offering(&sample_offering())
            .expect("append offering");
        store
            .append_worker_profile(&sample_worker())
            .expect("append worker");
    }

    fn append_public_snapshot_chain(store: &PublicIndexStore) {
        append_public_identity_chain(store);
        store
            .append_evidence(&sample_evidence())
            .expect("append evidence");
        store
            .append_snapshot(&sample_snapshot())
            .expect("append snapshot");
    }

    fn append_offering_revision_chain(store: &PublicIndexStore) -> Vec<ProviderOfferingRecord> {
        store
            .append_model_release(&sample_model())
            .expect("append model");
        let original = sample_offering();
        store
            .append_provider_offering(&original)
            .expect("append original offering");

        let mut first_revision = original.clone();
        first_revision.id = OfferingId("offering:revision-1".to_owned());
        first_revision.supersedes_offering_id = Some(original.id.clone());
        first_revision.effective_from_epoch_ms += 1_000;
        first_revision.fixed_request_micros = 10;
        store
            .append_provider_offering(&first_revision)
            .expect("append first revision");

        let mut second_revision = first_revision.clone();
        second_revision.id = OfferingId("offering:revision-2".to_owned());
        second_revision.supersedes_offering_id = Some(first_revision.id.clone());
        second_revision.effective_from_epoch_ms += 1_000;
        second_revision.fixed_request_micros = 20;
        store
            .append_provider_offering(&second_revision)
            .expect("append second revision");

        vec![original, first_revision, second_revision]
    }

    fn insert_snapshot_without_validation(store: &PublicIndexStore, snapshot: &SnapshotRecord) {
        store
            .connection
            .execute(
                "INSERT INTO snapshots (
                    id, created_at, ontology_version, source_revision, content_sha256,
                    model_release_ids_json, provider_offering_ids_json,
                    worker_profile_ids_json, evidence_ids_json,
                    model_release_count, provider_offering_count, worker_profile_count,
                    evidence_count
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    snapshot.id,
                    snapshot.created_at,
                    snapshot.ontology_version,
                    snapshot.source_revision,
                    snapshot.content_sha256,
                    serde_json::to_string(&snapshot.model_release_ids).expect("model ids"),
                    serde_json::to_string(&snapshot.provider_offering_ids).expect("offering ids"),
                    serde_json::to_string(&snapshot.worker_profile_ids).expect("worker ids"),
                    serde_json::to_string(&snapshot.evidence_ids).expect("evidence ids"),
                    i64::try_from(snapshot.model_release_count).expect("model count"),
                    i64::try_from(snapshot.provider_offering_count).expect("offering count"),
                    i64::try_from(snapshot.worker_profile_count).expect("worker count"),
                    i64::try_from(snapshot.evidence_count).expect("evidence count"),
                ],
            )
            .expect("unchecked snapshot fixture");
    }

    fn sample_quote() -> QuoteRecord {
        let selected_worker_id = WorkerId("worker:test".to_owned());
        QuoteRecord {
            decision_id: DecisionId("decision:test".to_owned()),
            task_id: TaskId("task:test".to_owned()),
            selected_worker_id: Some(selected_worker_id.clone()),
            selected_checker_worker_id: None,
            verification_policy: VerificationPolicy::Deterministic,
            evidence_snapshot_id: "snapshot:test".to_owned(),
            policy_version: "policy:1".to_owned(),
            expected_cash_micros: Some(12_500),
            expected_quota_milliunits: Some(1_000),
            expected_success_probability: Some(0.82),
            p95_latency_ms: Some(3_000),
            eligible_candidates: vec![CandidateQuoteAuditRecord {
                rank: 1,
                worker_id: selected_worker_id.clone(),
                checker_worker_id: None,
                success_mean: 0.82,
                success_lower_bound: 0.75,
                p95_latency_ms: 3_000,
                expected_cash_micros: 12_500,
                expected_quota_milliunits: 1_000,
                expected_accepted_cost_micros: 13_000,
                pareto_efficient: true,
                cost_breakdown: serde_json::json!({
                    "currency": "USD",
                    "expected_cash_micros": 12_500,
                    "expected_quota_milliunits": 1_000,
                    "expected_accepted_cost_micros": 13_000
                }),
            }],
            rejected_candidates: vec![RejectedCandidateAuditRecord {
                worker_id: WorkerId("worker:rejected".to_owned()),
                reasons: vec![serde_json::json!({"code": "cash_budget_exceeded"})],
            }],
            pareto_worker_ids: vec![selected_worker_id],
            selection_explanation: Some(SelectionExplanationAuditRecord {
                objective: "minimize confidence-bounded expected accepted cost".to_owned(),
                eligible_candidate_count: 1,
                tie_break_order: vec!["expected_accepted_cost_micros".to_owned()],
            }),
            created_at: "2026-08-04T00:00:00Z".to_owned(),
            request_fingerprint: DIGEST_A.to_owned(),
        }
    }

    fn sample_outcome(decision_id: Option<DecisionId>) -> PrivateOutcomeRecord {
        PrivateOutcomeRecord {
            decision_id,
            event: OutcomeEvent {
                id: "outcome:test".to_owned(),
                task_id: TaskId("task:test".to_owned()),
                worker_id: WorkerId("worker:test".to_owned()),
                skill_id: SkillId("skill:rust".to_owned()),
                accepted: true,
                validation_kind: ValidationKind::Deterministic,
                actual_cash_micros: 11_000,
                actual_quota_milliunits: 1_000,
                latency_ms: 2_500,
                observed_at: "2026-08-04T00:01:00Z".to_owned(),
                repository_scope: Some("repo-hash".to_owned()),
                metadata: Value::Object(serde_json::Map::new()),
            },
            checker_worker_id: None,
        }
    }

    fn temporary_database_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "open-workforce-{label}-{}-{nonce}.sqlite",
            std::process::id()
        ))
    }

    fn remove_sqlite_files(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut candidate = path.as_os_str().to_owned();
            candidate.push(suffix);
            match fs::remove_file(PathBuf::from(candidate)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("cleanup failed: {error}"),
            }
        }
    }
}

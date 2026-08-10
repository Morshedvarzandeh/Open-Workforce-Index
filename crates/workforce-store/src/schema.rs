//! The DDL. Moved byte for byte; a single altered character here changes
//! an append-only trigger or a CHECK constraint with no compiler complaint.

pub(crate) const IDENTITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS workforce_store_identity (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    kind TEXT NOT NULL CHECK (kind IN ('public_index', 'private_local'))
) STRICT;

CREATE TRIGGER IF NOT EXISTS workforce_store_identity_no_update
BEFORE UPDATE ON workforce_store_identity
BEGIN
    SELECT RAISE(ABORT, 'store identity is immutable');
END;

CREATE TRIGGER IF NOT EXISTS workforce_store_identity_no_delete
BEFORE DELETE ON workforce_store_identity
BEGIN
    SELECT RAISE(ABORT, 'store identity is immutable');
END;
"#;

pub(crate) const PUBLIC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS model_releases (
    id TEXT PRIMARY KEY,
    developer TEXT NOT NULL CHECK (length(trim(developer)) > 0),
    model_family TEXT NOT NULL CHECK (length(trim(model_family)) > 0),
    released_at TEXT NOT NULL CHECK (length(trim(released_at)) > 0),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens >= 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    artifact_sha256 TEXT NOT NULL CHECK (
        length(artifact_sha256) = 64
        AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0)
) STRICT;

CREATE TABLE IF NOT EXISTS provider_offerings (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (length(trim(provider)) > 0),
    supersedes_offering_id TEXT UNIQUE,
    effective_from_epoch_ms INTEGER NOT NULL,
    effective_until_epoch_ms INTEGER,
    currency TEXT NOT NULL CHECK (length(trim(currency)) > 0),
    input_micros_per_million_tokens INTEGER NOT NULL
        CHECK (input_micros_per_million_tokens >= 0),
    output_micros_per_million_tokens INTEGER NOT NULL
        CHECK (output_micros_per_million_tokens >= 0),
    fixed_request_micros INTEGER NOT NULL CHECK (fixed_request_micros >= 0),
    quota_milliunits_per_request INTEGER NOT NULL
        CHECK (quota_milliunits_per_request >= 0),
    context_window_tokens INTEGER NOT NULL CHECK (context_window_tokens >= 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    CHECK (
        effective_until_epoch_ms IS NULL
        OR effective_until_epoch_ms > effective_from_epoch_ms
    ),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT,
    FOREIGN KEY (supersedes_offering_id)
        REFERENCES provider_offerings(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS offerings_by_release_and_time
ON provider_offerings(
    model_release_id, effective_from_epoch_ms, effective_until_epoch_ms
);

CREATE TRIGGER IF NOT EXISTS offering_revision_matches_predecessor
BEFORE INSERT ON provider_offerings
WHEN NEW.supersedes_offering_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM provider_offerings AS predecessor
    WHERE predecessor.id = NEW.supersedes_offering_id
      AND predecessor.model_release_id = NEW.model_release_id
      AND predecessor.provider = NEW.provider
      AND predecessor.effective_from_epoch_ms <= NEW.effective_from_epoch_ms
      AND (
          predecessor.effective_until_epoch_ms IS NULL
          OR predecessor.effective_until_epoch_ms <= NEW.effective_from_epoch_ms
      )
)
BEGIN
    SELECT RAISE(
        ABORT,
        'offering revision must preserve provider and release and move time forward'
    );
END;

CREATE TABLE IF NOT EXISTS worker_profiles (
    id TEXT PRIMARY KEY,
    offering_id TEXT NOT NULL,
    harness_id TEXT NOT NULL CHECK (length(trim(harness_id)) > 0),
    harness_version TEXT NOT NULL CHECK (length(trim(harness_version)) > 0),
    reasoning_configuration TEXT NOT NULL
        CHECK (length(trim(reasoning_configuration)) > 0),
    system_prompt_sha256 TEXT NOT NULL CHECK (
        length(system_prompt_sha256) = 64
        AND system_prompt_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    skill_pack_version TEXT NOT NULL CHECK (length(trim(skill_pack_version)) > 0),
    toolset_version TEXT NOT NULL CHECK (length(trim(toolset_version)) > 0),
    execution_policy_sha256 TEXT NOT NULL CHECK (
        length(execution_policy_sha256) = 64
        AND execution_policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    supported_skill_ids_json TEXT NOT NULL CHECK (
        json_valid(supported_skill_ids_json)
        AND json_type(supported_skill_ids_json) = 'array'
    ),
    tools_json TEXT NOT NULL CHECK (
        json_valid(tools_json) AND json_type(tools_json) = 'array'
    ),
    privacy_clearance TEXT NOT NULL CHECK (privacy_clearance IN (
        'public', 'private_metadata', 'confidential_content', 'secret'
    )),
    configuration_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(configuration_sha256) = 64
        AND configuration_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0),
    FOREIGN KEY (offering_id) REFERENCES provider_offerings(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE IF NOT EXISTS evidence_observations (
    id TEXT PRIMARY KEY,
    model_release_id TEXT NOT NULL,
    worker_id TEXT,
    skill_id TEXT NOT NULL,
    benchmark_id TEXT NOT NULL CHECK (length(trim(benchmark_id)) > 0),
    evidence_tier TEXT NOT NULL CHECK (evidence_tier IN (
        'project_reproduced', 'independent_signed',
        'community_reproducible', 'vendor_reported'
    )),
    raw_score REAL NOT NULL,
    metric TEXT NOT NULL CHECK (length(trim(metric)) > 0),
    unit TEXT NOT NULL CHECK (length(trim(unit)) > 0),
    normalized_score REAL CHECK (
        normalized_score IS NULL OR
        (normalized_score >= 0.0 AND normalized_score <= 1.0)
    ),
    adapter_version TEXT NOT NULL CHECK (length(trim(adapter_version)) > 0),
    sample_count INTEGER CHECK (sample_count IS NULL OR sample_count >= 1),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    artifact_sha256 TEXT NOT NULL CHECK (
        length(artifact_sha256) = 64
        AND artifact_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    license TEXT NOT NULL CHECK (length(trim(license)) > 0),
    FOREIGN KEY (model_release_id) REFERENCES model_releases(id) ON DELETE RESTRICT,
    FOREIGN KEY (worker_id) REFERENCES worker_profiles(id) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER IF NOT EXISTS evidence_worker_release_matches
BEFORE INSERT ON evidence_observations
WHEN NEW.worker_id IS NOT NULL AND NOT EXISTS (
    SELECT 1
    FROM worker_profiles AS worker
    JOIN provider_offerings AS offering ON offering.id = worker.offering_id
    WHERE worker.id = NEW.worker_id
      AND offering.model_release_id = NEW.model_release_id
)
BEGIN
    SELECT RAISE(ABORT, 'evidence worker and model release do not match');
END;

CREATE INDEX IF NOT EXISTS evidence_by_worker_skill
ON evidence_observations(worker_id, skill_id, observed_at);

CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    ontology_version TEXT NOT NULL CHECK (length(trim(ontology_version)) > 0),
    source_revision TEXT NOT NULL CHECK (length(trim(source_revision)) > 0),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    model_release_ids_json TEXT NOT NULL CHECK (
        json_valid(model_release_ids_json)
        AND json_type(model_release_ids_json) = 'array'
    ),
    provider_offering_ids_json TEXT NOT NULL CHECK (
        json_valid(provider_offering_ids_json)
        AND json_type(provider_offering_ids_json) = 'array'
    ),
    worker_profile_ids_json TEXT NOT NULL CHECK (
        json_valid(worker_profile_ids_json)
        AND json_type(worker_profile_ids_json) = 'array'
    ),
    evidence_ids_json TEXT NOT NULL CHECK (
        json_valid(evidence_ids_json) AND json_type(evidence_ids_json) = 'array'
    ),
    model_release_count INTEGER NOT NULL CHECK (model_release_count >= 0),
    provider_offering_count INTEGER NOT NULL CHECK (provider_offering_count >= 0),
    worker_profile_count INTEGER NOT NULL CHECK (worker_profile_count >= 0),
    evidence_count INTEGER NOT NULL CHECK (evidence_count >= 0),
    CHECK (json_array_length(model_release_ids_json) = model_release_count),
    CHECK (json_array_length(provider_offering_ids_json) = provider_offering_count),
    CHECK (json_array_length(worker_profile_ids_json) = worker_profile_count),
    CHECK (json_array_length(evidence_ids_json) = evidence_count)
) STRICT;

CREATE TRIGGER IF NOT EXISTS model_releases_no_update
BEFORE UPDATE ON model_releases BEGIN
    SELECT RAISE(ABORT, 'model releases are append-only');
END;
CREATE TRIGGER IF NOT EXISTS model_releases_no_delete
BEFORE DELETE ON model_releases BEGIN
    SELECT RAISE(ABORT, 'model releases are append-only');
END;
CREATE TRIGGER IF NOT EXISTS provider_offerings_no_update
BEFORE UPDATE ON provider_offerings BEGIN
    SELECT RAISE(ABORT, 'provider offerings are append-only');
END;
CREATE TRIGGER IF NOT EXISTS provider_offerings_no_delete
BEFORE DELETE ON provider_offerings BEGIN
    SELECT RAISE(ABORT, 'provider offerings are append-only');
END;
CREATE TRIGGER IF NOT EXISTS worker_profiles_no_update
BEFORE UPDATE ON worker_profiles BEGIN
    SELECT RAISE(ABORT, 'worker profiles are append-only');
END;
CREATE TRIGGER IF NOT EXISTS worker_profiles_no_delete
BEFORE DELETE ON worker_profiles BEGIN
    SELECT RAISE(ABORT, 'worker profiles are append-only');
END;
CREATE TRIGGER IF NOT EXISTS evidence_observations_no_update
BEFORE UPDATE ON evidence_observations BEGIN
    SELECT RAISE(ABORT, 'evidence observations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS evidence_observations_no_delete
BEFORE DELETE ON evidence_observations BEGIN
    SELECT RAISE(ABORT, 'evidence observations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS snapshots_no_update
BEFORE UPDATE ON snapshots BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;
CREATE TRIGGER IF NOT EXISTS snapshots_no_delete
BEFORE DELETE ON snapshots BEGIN
    SELECT RAISE(ABORT, 'snapshots are append-only');
END;

PRAGMA user_version = 2;
"#;

pub(crate) const PRIVATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS routing_quotes (
    decision_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    selected_worker_id TEXT,
    selected_checker_worker_id TEXT,
    verification_policy TEXT NOT NULL CHECK (verification_policy IN (
        'deterministic', 'maker_checker', 'human_approval'
    )),
    evidence_snapshot_id TEXT NOT NULL,
    policy_version TEXT NOT NULL CHECK (length(trim(policy_version)) > 0),
    expected_cash_micros INTEGER CHECK (
        expected_cash_micros IS NULL OR expected_cash_micros >= 0
    ),
    expected_quota_milliunits INTEGER CHECK (
        expected_quota_milliunits IS NULL OR expected_quota_milliunits >= 0
    ),
    expected_success_probability REAL CHECK (
        expected_success_probability IS NULL OR
        (expected_success_probability >= 0.0 AND expected_success_probability <= 1.0)
    ),
    p95_latency_ms INTEGER CHECK (p95_latency_ms IS NULL OR p95_latency_ms >= 0),
    eligible_candidates_json TEXT NOT NULL CHECK (
        json_valid(eligible_candidates_json)
        AND json_type(eligible_candidates_json) = 'array'
    ),
    rejected_candidates_json TEXT NOT NULL CHECK (
        json_valid(rejected_candidates_json)
        AND json_type(rejected_candidates_json) = 'array'
    ),
    pareto_worker_ids_json TEXT NOT NULL CHECK (
        json_valid(pareto_worker_ids_json)
        AND json_type(pareto_worker_ids_json) = 'array'
    ),
    selection_explanation_json TEXT CHECK (
        selection_explanation_json IS NULL OR (
            json_valid(selection_explanation_json)
            AND json_type(selection_explanation_json) = 'object'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    request_fingerprint TEXT NOT NULL CHECK (
        length(request_fingerprint) = 64
        AND request_fingerprint NOT GLOB '*[^0-9a-f]*'
    ),
    CHECK (
        selected_checker_worker_id IS NULL
        OR selected_checker_worker_id <> selected_worker_id
    ),
    CHECK (
        (
            selected_worker_id IS NULL
            AND selected_checker_worker_id IS NULL
            AND expected_cash_micros IS NULL
            AND expected_quota_milliunits IS NULL
            AND expected_success_probability IS NULL
            AND p95_latency_ms IS NULL
            AND selection_explanation_json IS NULL
        ) OR (
            selected_worker_id IS NOT NULL
            AND expected_cash_micros IS NOT NULL
            AND expected_quota_milliunits IS NOT NULL
            AND expected_success_probability IS NOT NULL
            AND p95_latency_ms IS NOT NULL
            AND selection_explanation_json IS NOT NULL
        )
    ),
    UNIQUE (decision_id, task_id, selected_worker_id)
) STRICT;

CREATE TABLE IF NOT EXISTS outcome_events (
    id TEXT PRIMARY KEY,
    decision_id TEXT,
    task_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    skill_id TEXT NOT NULL,
    accepted INTEGER NOT NULL CHECK (accepted IN (0, 1)),
    validation_kind TEXT NOT NULL CHECK (validation_kind IN (
        'deterministic', 'human', 'independent_model', 'self_reported'
    )),
    actual_cash_micros INTEGER NOT NULL CHECK (actual_cash_micros >= 0),
    actual_quota_milliunits INTEGER NOT NULL CHECK (actual_quota_milliunits >= 0),
    latency_ms INTEGER NOT NULL CHECK (latency_ms >= 0),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    repository_scope TEXT,
    metadata_json TEXT NOT NULL CHECK (json_valid(metadata_json)),
    checker_worker_id TEXT,
    CHECK (checker_worker_id IS NULL OR checker_worker_id <> worker_id),
    FOREIGN KEY (decision_id, task_id, worker_id)
        REFERENCES routing_quotes(decision_id, task_id, selected_worker_id)
        ON DELETE RESTRICT
) STRICT;

CREATE INDEX IF NOT EXISTS outcomes_by_worker_skill
ON outcome_events(worker_id, skill_id, observed_at);

CREATE TRIGGER IF NOT EXISTS routing_quotes_no_update
BEFORE UPDATE ON routing_quotes BEGIN
    SELECT RAISE(ABORT, 'routing quotes are append-only');
END;
CREATE TRIGGER IF NOT EXISTS routing_quotes_no_delete
BEFORE DELETE ON routing_quotes BEGIN
    SELECT RAISE(ABORT, 'routing quotes are append-only');
END;
CREATE TRIGGER IF NOT EXISTS outcome_events_no_update
BEFORE UPDATE ON outcome_events BEGIN
    SELECT RAISE(ABORT, 'outcome events are append-only');
END;
CREATE TRIGGER IF NOT EXISTS outcome_events_no_delete
BEFORE DELETE ON outcome_events BEGIN
    SELECT RAISE(ABORT, 'outcome events are append-only');
END;

PRAGMA user_version = 2;
"#;

"""Hiring a model that runs on your own machine.

Every other worker on the roster arrives by *import*: a published price list
is fetched, hashed, and parsed, and the numbers in the index are traceable to
bytes somebody else published. A model on your desktop has no such list. There
is no upstream row for `ollama/hermes3`, and inventing a URL to point at would
be the one dishonest thing this project cannot afford.

So a local model is *declared* instead, and the declaration says so. The two
facts a declaration asserts are the two a local model actually makes true:

  * the price is zero, because the tokens are yours; and
  * the context window is whatever the model card says, which is the only
    number here the declarer is trusted for.

Ability is not declared. A hired local model starts with the same assumed,
`vendor_reported`, `example.invalid` evidence every seeded worker starts with,
discounted to a tenth of its weight for exactly that reason. It has to earn
its posterior from your own verified outcomes like everyone else.

What a local model uniquely brings is *clearance*. Cloud workers carry
`private_metadata`; a model that never leaves the machine is the only kind
that can hold `confidential_content`. That is the whole reason to hire one,
and it is why a long context window matters more here than anywhere else: a
confidential document that exceeds every cleared worker's window has nobody
to do it, at any price.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

# The empty-string SHA-256, used wherever a component is genuinely absent —
# there is no system prompt and no execution policy behind a bare `ollama run`.
EMPTY_SHA256 = ("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b78"
                "52b855")

# The same assumed-ability row every seeded worker carries. Reusing the exact
# benchmark id, tier, source and digest is deliberate: a hired local model must
# be indistinguishable from any other unmeasured worker, so nothing about being
# local reads as an endorsement.
ASSUMED_BENCHMARK = "benchmark:assumed-for-demonstration"
ASSUMED_SOURCE = "https://example.invalid/assumed-not-measured"
ASSUMED_ARTIFACT = ("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c1"
                    "5b0f00a08")

# Role -> the harness identity, skill and tools that role means. Copied from
# the seeded roster's own conventions so a hired worker is the same shape as a
# worker that arrived by import. The identities must differ per role: the
# store keys a worker by its configuration digest, and two roles that share a
# harness, skill pack and toolset are literally the same configuration.
ROLES = {
    "text": {
        "harness_id": "owi-text-agent",
        "skill_pack_version": "text-v1",
        "toolset_version": "none-v1",
        "skill_id": "skill:text-editing",
        "tools": [],
        "assumed_score": 78.0,
    },
    "extract": {
        "harness_id": "owi-extraction-agent",
        "skill_pack_version": "extract-v1",
        "toolset_version": "json-schema-v1",
        "skill_id": "skill:structured-extraction",
        "tools": ["json-schema-validator"],
        "assumed_score": 74.0,
    },
    "code": {
        "harness_id": "owi-code-agent",
        "skill_pack_version": "code-v1",
        "toolset_version": "shell-v1",
        "skill_id": "skill:python-numerical-implementation",
        "tools": ["shell"],
        "assumed_score": 70.0,
    },
    "plan": {
        "harness_id": "owi-planning-agent",
        "skill_pack_version": "plan-v1",
        "toolset_version": "none-v1",
        "skill_id": "skill:planning-decomposition",
        "tools": [],
        "assumed_score": 72.0,
    },
}


def configuration_sha256(model_release_id: str, offering_id: str,
                         provider: str, profile: dict) -> str:
    """The store's own configuration key, recomputed here.

    The digest is length-prefixed per component so no two different component
    lists can concatenate to the same string. The store rejects any profile
    whose digest it cannot reproduce, which is what makes this a check rather
    than a formality: get it wrong and the append fails loudly.
    """
    components = [
        model_release_id, offering_id, provider,
        profile["harness_id"], profile["harness_version"],
        profile["reasoning_configuration"], profile["system_prompt_sha256"],
        profile["skill_pack_version"], profile["toolset_version"],
        profile["execution_policy_sha256"],
    ]
    key = "".join(f"{len(c)}:{c}" for c in components)
    return hashlib.sha256(key.encode()).hexdigest()


def declaration(model: str, context_window: int, roles: list[str],
                worker_prefix: str, developer: str, recorded_at: str,
                snapshot_id: str, at_epoch_ms: int,
                provider: str = "ollama") -> dict:
    """An IndexSeed that hires `model` as a set of confidential-cleared roles.

    Returned rather than written so the caller can show it before applying it.
    Nothing here is hidden: the file that lands in the index is the file the
    user can read, diff and delete.
    """
    unknown = [r for r in roles if r not in ROLES]
    if unknown:
        raise ValueError(f"unknown role(s): {', '.join(sorted(unknown))}; "
                         f"known: {', '.join(sorted(ROLES))}")
    if context_window <= 0:
        raise ValueError("context window must be a positive number of tokens")

    model_release_id = f"model:{provider}/{model}"
    offering_id = f"offering:{provider}/{model}"
    # The one string in this file that is not a fact about the model is the
    # source, so it says exactly what it is instead of imitating a URL.
    source = f"local declaration: {provider} pull {model}"

    profiles, evidence = [], []
    for role in roles:
        spec = ROLES[role]
        profile = {
            "id": f"worker:{worker_prefix}/{role}",
            "offering_id": offering_id,
            "harness_id": spec["harness_id"],
            "harness_version": "1.0.0",
            "reasoning_configuration": "standard",
            "system_prompt_sha256": EMPTY_SHA256,
            "skill_pack_version": spec["skill_pack_version"],
            "toolset_version": spec["toolset_version"],
            "execution_policy_sha256": EMPTY_SHA256,
            "supported_skill_ids": [spec["skill_id"]],
            "tools": list(spec["tools"]),
            # The entire point of hiring locally.
            "privacy_clearance": "confidential_content",
            "recorded_at": recorded_at,
        }
        profile["configuration_sha256"] = configuration_sha256(
            model_release_id, offering_id, provider, profile)
        profiles.append(profile)
        evidence.append({
            "id": f"evidence:assumed:{worker_prefix}:{role}",
            "model_release_id": model_release_id,
            "worker_id": profile["id"],
            "skill_id": spec["skill_id"],
            "benchmark_id": ASSUMED_BENCHMARK,
            "evidence_tier": "vendor_reported",
            "raw_score": spec["assumed_score"],
            "metric": "pass_rate",
            "unit": "percent",
            "normalized_score": spec["assumed_score"] / 100.0,
            "adapter_version": "assumed@0",
            "sample_count": 20,
            "observed_at": recorded_at,
            "source_url": ASSUMED_SOURCE,
            "artifact_sha256": ASSUMED_ARTIFACT,
            "license": "CC0-1.0",
        })

    return {
        "_comment": [
            f"{provider}/{model} declared, not imported: it has no published",
            "price row to fetch, and the honest reason is that its price is",
            "zero. Context window is the model card's. Ability is ASSUMED and",
            "weighted accordingly -- replace it by measuring, not by editing.",
        ],
        "snapshot_id": snapshot_id,
        "created_at": recorded_at,
        "ontology_version": "ontology:v1",
        "source_revision": f"local-declaration:{provider}/{model}",
        "model_releases": [{
            "id": model_release_id,
            "developer": developer,
            "model_family": f"{provider}/{model}",
            "released_at": "unknown",
            "context_window_tokens": context_window,
            "source_url": source,
            "artifact_sha256": EMPTY_SHA256,
            "recorded_at": recorded_at,
        }],
        "provider_offerings": [{
            "id": offering_id,
            "model_release_id": model_release_id,
            "provider": provider,
            "currency": "USD",
            # Zero is a measurement here, not a placeholder: the tokens are
            # yours. What a local run actually costs shows up as latency and
            # as the expected price of a retry, both of which the allocator
            # already charges for.
            "input_micros_per_million_tokens": 0,
            "output_micros_per_million_tokens": 0,
            "fixed_request_micros": 0,
            "quota_milliunits_per_request": 0,
            "context_window_tokens": context_window,
            "effective_from_epoch_ms": at_epoch_ms,
            "effective_until_epoch_ms": None,
            "supersedes_offering_id": None,
            "source_url": source,
            "recorded_at": recorded_at,
        }],
        "worker_profiles": profiles,
        "evidence": evidence,
    }


# --- where a hired model is remembered ------------------------------------
#
# Snapshots are immutable: a snapshot id names a closed set of facts fixed at
# the moment it was written, which is what lets a recorded decision be replayed
# years later. Hiring therefore cannot edit a snapshot -- it cuts a new one.
# Something has to say which snapshot is current, and that is this pointer.
# Moving a pointer is not rewriting history; every decision still records the
# exact snapshot id it quoted.

POINTER = "snapshot.json"
LOCAL_MODELS = "local-models.json"
DEFAULT_SNAPSHOT = "snapshot:manager-scenario-v1"


def current_snapshot(home: Path) -> str:
    """The snapshot decisions should quote, or the seeded default."""
    path = Path(home) / POINTER
    if not path.exists():
        return DEFAULT_SNAPSHOT
    try:
        return json.loads(path.read_text())["snapshot_id"]
    except (json.JSONDecodeError, KeyError, TypeError):
        # A broken pointer must not silently route decisions at a snapshot
        # nobody chose; falling back to the seeded default is the one
        # behaviour that is both defined and visible.
        return DEFAULT_SNAPSHOT


def set_snapshot(home: Path, snapshot_id: str) -> None:
    (Path(home) / POINTER).write_text(
        json.dumps({"snapshot_id": snapshot_id}, indent=2) + "\n")


def local_models(home: Path) -> dict[str, str]:
    """Hired local models: owi-do model name -> the ollama model to run."""
    path = Path(home) / LOCAL_MODELS
    if not path.exists():
        return {}
    try:
        loaded = json.loads(path.read_text())
    except json.JSONDecodeError:
        return {}
    return {k: v for k, v in loaded.items()
            if isinstance(k, str) and isinstance(v, str) and not
            k.startswith("_")}


def remember_local_model(home: Path, name: str, target: str) -> None:
    path = Path(home) / LOCAL_MODELS
    known = local_models(home)
    known[name] = target
    path.write_text(json.dumps(known, indent=2) + "\n")

# --- what the index already knows -----------------------------------------


def existing_offering(index: Path, offering_id: str) -> dict | None:
    """The offering already in the index under this id, if any.

    Hiring must never quietly overwrite an imported fact with a declared one.
    A published price row and a local declaration are different kinds of
    claim, and the imported one won the argument by having a source.
    """
    import sqlite3
    if not Path(index).exists():
        return None
    connection = sqlite3.connect(f"file:{index}?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT context_window_tokens, input_micros_per_million_tokens, "
            "       output_micros_per_million_tokens, source_url "
            "FROM provider_offerings WHERE id = ?", (offering_id,)).fetchone()
    except sqlite3.DatabaseError:
        return None
    finally:
        connection.close()
    if row is None:
        return None
    return {"context_window_tokens": row[0],
            "input_micros_per_million_tokens": row[1],
            "output_micros_per_million_tokens": row[2],
            "source_url": row[3]}


def existing_workers(index: Path, worker_ids: list[str]) -> list[str]:
    import sqlite3
    if not Path(index).exists() or not worker_ids:
        return []
    connection = sqlite3.connect(f"file:{index}?mode=ro", uri=True)
    try:
        placeholders = ",".join("?" * len(worker_ids))
        rows = connection.execute(
            f"SELECT id FROM worker_profiles WHERE id IN ({placeholders})",
            worker_ids).fetchall()
    except sqlite3.DatabaseError:
        return []
    finally:
        connection.close()
    return sorted(row[0] for row in rows)

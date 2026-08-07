# ADR 0007: Preserve environmental boundaries and counterfactual uncertainty

- Status: Accepted
- Date: 2026-08-07

## Context

Users want to see the cash, energy, greenhouse-gas, and water consequences of
AI work for each private Git project, and how much an optimizer avoided relative
to their previous policy. These quantities arrive at different resolutions.
Provider billing can identify an invocation while environmental disclosures may
describe a provider product, model benchmark, data-center region, or median
prompt. Serving hardware, utilization, region, and time are commonly hidden.

Those differences are material. Google reports that a production inference
boundary including accelerators, host CPU and memory, idle capacity, and
facility overhead is 2.4 times its accelerator-only result for the studied
Gemini Apps prompt. Its disclosed carbon result is market-based, its water
result is direct cooling consumption, and its study excludes training, network,
and end-user devices. The figures are therefore not universal per-model
constants. See [Google's production methodology][google-ai].

The GHG Protocol requires location-based and market-based electricity results
to remain distinct. Its Product Standard also requires a consistent functional
unit for performance tracking, disclosure of uncertainty and allocation
choices, and separate reporting of avoided emissions rather than deducting them
from an inventory. Water withdrawal and water consumption likewise describe
different flows. See [GHG Protocol Scope 2 Guidance][scope2], the
[GHG Protocol Product Standard][product-standard], and [USGS water-use
terminology][usgs-water].

## Decision

### Environmental evidence belongs to an offering or deployment

An `EnvironmentalProfile` is immutable, time-bounded evidence for one exact
provider offering and one declared functional unit. It is not a score attached
to a mutable model-family name. The profile records its measurement boundary,
source provenance, applicable dates, and one or more rational `ImpactTerm`
coefficients.

An impact term identifies all of the following:

1. one metric;
2. one lifecycle phase;
3. one activity unit;
4. one result unit; and
5. a non-negative integer numerator and a strictly positive integer
   denominator.

Initial public metrics are:

- energy in micro-watt-hours (`uWh`);
- location-based CO2e in micrograms (`ugCO2e`);
- market-based CO2e in micrograms (`ugCO2e`);
- water consumption in microliters (`uL`); and
- water withdrawal in microliters (`uL`).

Initial lifecycle phases are inference operations, inference embodied hardware,
inference embodied facility, training operations, and training embodied impact.
Location-based and market-based CO2e are alternative accounting views and are
never added. Water consumption and withdrawal are never substituted or netted.

A profile can state an activity basis such as request, input token, output
token, or GPU-second only when the source supports that scaling rule. A
provider's median-prompt disclosure remains a median-prompt functional unit; it
is not converted into a per-token factor by OWI.

### Unknown is absence or explicit state, never numeric zero

Missing environmental evidence remains unknown. A missing impact term means
that the public index has no usable factor for that metric and boundary. A
factor numerator may be zero when the source defensibly measured zero; its
denominator is strictly positive. Presence or explicit estimate status, rather
than numeric value, distinguishes known zero from unknown.

The planned private `ImpactEstimate` projection has an explicit `KnownEstimate`
or `UnknownEstimate` status. A known estimate carries a non-negative numeric
value and no unknown reason. An unknown estimate carries a structured reason
and no numeric value. A measured zero can therefore remain a known zero without
being confused with missing evidence.

Common unknown reasons include provider non-disclosure, hidden serving region,
hidden hardware, no compatible factor, incompatible boundary, stale evidence,
restricted source licensing, and incomplete billing reconciliation.

Unknown impact does not make a candidate environmentally free. An
environment-aware routing policy must explicitly choose one of three behaviors:

- exclude candidates without adequate evidence;
- apply a separately labeled conservative scenario bound; or
- omit the environmental objective and report the coverage gap.

It must not rank an unknown value as zero.

### Calculation and lifecycle rules

When a source publishes an affine token model, integer checked arithmetic may
calculate IT energy as:

```text
E_it = e_request
     + input_tokens * e_input
     + output_tokens * e_output
     + cache_read_tokens * e_cache_read
     + cache_write_tokens * e_cache_write
```

Facility overhead is applied only when the source boundary excludes it:

```text
E_facility = E_it * PUE
```

A full-stack provider measurement must not receive another PUE or generic
overhead multiplier. Operational carbon is calculated separately for each
available method:

```text
CO2e_location = E_facility_kWh * location_based_factor
CO2e_market   = E_facility_kWh * market_based_factor
```

Marginal grid emissions are consequential scenario evidence. They are not
included in the attributional location-based or market-based inventory.

Water factors declare their energy denominator and boundary. Site consumption,
electricity-supply consumption, and electricity-supply withdrawal remain
separate components. The Green Grid defines site WUE and source WUE relative to
IT equipment energy and explicitly limits WUE to operations rather than the
full equipment lifecycle. See [The Green Grid WUE methodology][wue].

For a self-hosted asset with a suitable lifecycle inventory, embodied impact is
first allocated to the reporting period over its expected service life, then to
the service using a physical resource-time or reserved-capacity share, and only
then to requests. Idle capacity remains visible. Economic allocation is a
documented fallback, not the default.

For a hosted API, OWI does not fabricate hardware or training allocations when
the provider has not published them. A model's training total is reported at
model level unless the source supplies a defensible allocation denominator. It
is not arbitrarily amortized over projected lifetime requests.

### Project inventory and coverage

The private usage ledger and project allocations defined by ADR 0006 are the
activity-data source. Every impact calculation references the exact usage,
environmental profile, factor snapshot, calculator version, and canonical
calculation digest. Reports aggregate from those calculation records, not from
rounded presentation values.

Each project report shows:

- marginal, billed, and allocated fixed cost separately;
- requests, tokens, quota, retries, checks, tools, and accepted outcomes;
- energy, location-based CO2e, market-based CO2e, water consumption, and water
  withdrawal separately;
- operational and embodied components separately;
- coverage by invocation and activity quantity; and
- unknown counts grouped by reason.

A numeric environmental subtotal is labeled partial when it excludes unknown
usage. Absolute totals are shown with intensity per verified accepted task so a
lower per-task footprint cannot hide increasing total consumption.

### Savings are a separate counterfactual ledger

A `Baseline` is frozen before the compared work and names the project,
functional unit, task and quality contract, policy digest, verification and
fallback policy, price snapshot, environmental snapshot, and validity period.
Supported baseline designs include a prior production router, a fixed
business-as-usual offering, a shadow policy, and a randomized control.

The preferred functional unit is a verified accepted task at the same risk and
quality threshold. Optimized actuals include planner, router, maker, retry,
checker, verifier, and tool usage. Failed work remains in project inventory even
when it cannot support an equivalent-outcome savings comparison.

For a comparable work unit:

```text
estimated avoided cash   = baseline counterfactual cash - actual cash
estimated avoided impact = baseline counterfactual impact - actual impact
```

Differences are signed; negative savings remain visible. Cash, quota, CO2e, and
water differences are separate. Subscription quota preserved is not cash saved
unless it changes billed cash.

The report states baseline design, eligible and excluded tasks, coverage,
uncertainty interval, and probability of a positive difference when these are
available. Shared factors are treated as correlated during uncertainty
propagation. If methodology changes materially, both baseline and actual are
restated on the same basis or the comparison is discontinued.

Counterfactual environmental differences are labeled `estimated avoided
impact`. They are never subtracted from the actual inventory. A scenario such as
"the strongest model for every task" is not called a saving unless it was a
precommitted or historically evidenced business-as-usual policy.

### Provenance and evidence quality

Every source artifact records its URI, retrieval time, content digest,
publisher, publication time when available, methodology and version, license,
redistribution permission, assurance status, and whether it is provider
authored. Factor snapshots are append-only manifests; a weekly updater appends
and supersedes profiles rather than rewriting history.

Quality is assessed independently for technological, temporal, geographical,
completeness, and reliability dimensions. A qualitative grade does not create a
numeric confidence interval. Exact local measurement, provider allocation,
reproducible deployment benchmarks, peer-reviewed proxies, and provider
aggregate disclosures remain distinguishable evidence types.

Provider-aggregate disclosures are not silently transferred to API offerings.
Mistral, for example, describes its published lifecycle result as a first
approximation and notes the lack of a reliable public GPU lifecycle inventory.
See [Mistral's lifecycle disclosure][mistral].

### Public/private graph boundary

Only environmental profiles, impact terms, their exact offering links, and
public source provenance may enter the public index. Project identity, Git
fingerprints, calculated usage impact, baselines, savings, and project reports
remain private. SHACL targets every private reporting predicate directly and
rejects every resource typed with a private reporting class from a public
export. This validation is defense in depth; physical database separation
remains the security boundary.

The ontology defines private `Project`, `ImpactEstimate`, `Baseline`,
`SavingsEstimate`, and `ProjectReport` terms as a forward contract. The current
Rust and store crates do **not** yet materialize this RDF projection. The private
SQL schema, typed Rust events and calculations, factor ingestion, report
generator, and full SHACL execution are planned implementation work. Syntax
validity of the vocabulary must not be described as a working accounting
engine.

## Consequences

Environmental coverage will initially be incomplete for many hosted offerings,
and OWI will sometimes decline to make an environmental comparison. This is
preferable to false precision. Reports are larger because they preserve
boundaries, methods, provenance, and unknowns. In return, users can distinguish
measured project spend from chargeback, physical location-based emissions from
contractual market-based emissions, consumed water from withdrawn water, and
actual inventory from counterfactual optimizer benefit.

## Invariants

1. Unknown environmental impact is never represented by numeric zero.
2. Location-based and market-based CO2e are never added or silently substituted.
3. Water withdrawal and consumption are never collapsed into one value.
4. Every public factor declares a functional unit, boundary, lifecycle phase,
   units, non-negative rational numerator, positive denominator, exact
   offering, and provenance.
5. Provider/product aggregates are not transferred to exact offerings without
   explicit applicability evidence.
6. Operational, embodied, and training impacts remain separately inspectable.
7. Project reports disclose environmental coverage and excluded components.
8. Avoided impacts remain outside the actual inventory and retain their
   counterfactual label.
9. Comparisons use a precommitted baseline and an equivalent functional unit.
10. Private project, impact, baseline, savings, and report facts never enter a
    public snapshot.

[google-ai]: https://arxiv.org/html/2508.15734v1
[scope2]: https://ghgprotocol.org/sites/default/files/2023-03/Scope%202%20Guidance.pdf
[product-standard]: https://ghgprotocol.org/sites/default/files/standards/Product-Life-Cycle-Accounting-Reporting-Standard-EReader_041613_0.pdf
[usgs-water]: https://www.usgs.gov/mission-areas/water-resources/science/water-use-terminology
[wue]: https://www.thegreengrid.org/system/files/store/WUE_v1.pdf
[mistral]: https://mistral.ai/news/our-contribution-to-a-global-environmental-standard-for-ai/

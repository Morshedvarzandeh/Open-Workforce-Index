# Ontology competency questions

These questions define the minimum useful public knowledge-graph projection
plus clearly labeled planned local-only joins. The SQLite ledgers remain
authoritative; public queries run against an immutable RDF snapshot, while a
local view may add private task/decision nodes without exporting them. Every
query carries its own prefixes so it can be copied into a standards-compliant
SPARQL 1.1 processor without hidden prologue state.

## 1. Which exact workers can serve a given skill now?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT DISTINCT ?worker ?offering ?provider WHERE {
  ?worker a owf:WorkerProfile ;
          owf:hasSkill <https://openworkforce.dev/skill/code-review> ;
          owf:usesOffering ?offering .
  ?offering owf:offeredBy ?provider ; owf:validFrom ?start .
  FILTER (?start <= NOW())
  FILTER NOT EXISTS { ?offering owf:validUntil ?end . FILTER (?end <= NOW()) }
}
```

## 2. How is a worker distinct from its underlying model release?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?worker ?release ?harness (GROUP_CONCAT(STR(?tool); separator=", ") AS ?tools) WHERE {
  ?worker a owf:WorkerProfile ; owf:usesOffering/owf:offersRelease ?release ; owf:hasHarness ?harness .
  OPTIONAL { ?worker owf:supportsTool ?tool }
}
GROUP BY ?worker ?release ?harness
```

## 3. Which evidence supports an ability estimate?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX prov: <http://www.w3.org/ns/prov#>

SELECT ?estimate ?worker ?skill ?mean ?evidence ?source WHERE {
  ?estimate a owf:AbilityEstimate ;
            owf:abilityForWorker ?worker ; owf:abilitySkill ?skill ;
            owf:estimateMean ?mean ; owf:derivedFromEvidence ?evidence .
  ?evidence prov:wasDerivedFrom ?source .
}
```

## 4. Which quoted workers are cheapest for this application-specific task, with evidence trace?

This planned v0.2 query runs only in the local read model: the structured task,
quotes, and outcome history never enter the public graph. Replace the example
decision IRI with the quote set being inspected; this prevents costs from
different snapshots being mixed. `expectedAcceptedCostMicrounits` is the
allocator's already-computed comparable cost; SPARQL does not recreate the cost
model. The nested anti-joins require every declared skill and tool, an adequate
lower confidence bound, and evidence with the exact domain/task/artifact/
acceptance tuple. A task with no skill requirements still returns qualified
quoted workers with the evidence columns unbound.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX prov: <http://www.w3.org/ns/prov#>

SELECT DISTINCT ?worker ?expectedAcceptedCost ?skill ?lowerBound ?evidence ?source WHERE {
  VALUES ?decision { <https://example.org/private/decisions/decision-123> }

  ?decision a owf:RoutingDecision ;
            owf:decisionForTask ?task ;
            owf:hasCandidateQuote ?quote .
  ?task a owf:StructuredTaskSpec ;
        owf:taskHasClass ?taskClass ;
        owf:taskApplicationDomain ?domain ;
        owf:taskArtifactType ?artifact ;
        owf:taskAcceptanceProfile ?acceptance .
  ?quote owf:candidateWorker ?worker ;
         owf:expectedAcceptedCostMicrounits ?expectedAcceptedCost .

  FILTER NOT EXISTS {
    ?task owf:requiresTool ?requiredTool .
    FILTER NOT EXISTS { ?worker owf:supportsTool ?requiredTool }
  }
  FILTER NOT EXISTS {
    ?task owf:requiresSkill ?requirement .
    ?requirement owf:requiredSkill ?requiredSkill ;
                 owf:minimumQuality ?minimumQuality .
    FILTER NOT EXISTS {
      ?worker owf:hasSkill ?requiredSkill .
      ?qualifiedEstimate a owf:ScopedAbilityEstimate ;
                         owf:abilityForWorker ?worker ;
                         owf:abilitySkill ?requiredSkill ;
                         owf:estimateLowerBound ?qualifiedLower ;
                         owf:estimateApplicationDomain ?domain ;
                         owf:forTaskClass ?taskClass ;
                         owf:estimateArtifactType ?artifact ;
                         owf:estimateAcceptanceProfile ?acceptance ;
                         owf:derivedFromEvidence ?qualifiedEvidence .
      FILTER (?qualifiedLower >= ?minimumQuality)
      ?qualifiedEvidence a owf:CapabilityEvidenceObservation ;
                         owf:measuresSkill ?requiredSkill ;
                         owf:evidenceApplicationDomain ?domain ;
                         owf:forTaskClass ?taskClass ;
                         owf:evidenceArtifactType ?artifact ;
                         owf:evidenceAcceptanceProfile ?acceptance .
      FILTER NOT EXISTS {
        ?task owf:requiresTool ?evidenceTool .
        FILTER NOT EXISTS {
          ?qualifiedEstimate owf:derivedFromEvidence ?toolEvidence .
          ?toolEvidence a owf:CapabilityEvidenceObservation ;
                        owf:measuresSkill ?requiredSkill ;
                        owf:evidenceApplicationDomain ?domain ;
                        owf:forTaskClass ?taskClass ;
                        owf:evidenceArtifactType ?artifact ;
                        owf:evidenceAcceptanceProfile ?acceptance ;
                        owf:evaluatedWithTool ?evidenceTool .
        }
      }
    }
  }

  OPTIONAL {
    ?task owf:requiresSkill ?traceRequirement .
    ?traceRequirement owf:requiredSkill ?skill ;
                      owf:minimumQuality ?traceMinimum .
    ?traceEstimate a owf:ScopedAbilityEstimate ;
                   owf:abilityForWorker ?worker ;
                   owf:abilitySkill ?skill ;
                   owf:estimateLowerBound ?lowerBound ;
                   owf:estimateApplicationDomain ?domain ;
                   owf:forTaskClass ?taskClass ;
                   owf:estimateArtifactType ?artifact ;
                   owf:estimateAcceptanceProfile ?acceptance ;
                   owf:derivedFromEvidence ?evidence .
    FILTER (?lowerBound >= ?traceMinimum)
    ?evidence a owf:CapabilityEvidenceObservation ;
              owf:measuresSkill ?skill ;
              owf:evidenceApplicationDomain ?domain ;
              owf:forTaskClass ?taskClass ;
              owf:evidenceArtifactType ?artifact ;
              owf:evidenceAcceptanceProfile ?acceptance ;
              prov:wasDerivedFrom ?source .
  }
}
ORDER BY ?expectedAcceptedCost ?worker ?skill ?evidence
```

Because the applicability tuple and required tool context must match exactly,
a tool-free legal-factuality observation cannot qualify a worker for CAD model
creation. CAD is only one example; the same primitives cover code, research,
image generation, simulation, translation, support, and future task families.

## 5. How much does harness choice change observed performance for one release?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?harness (AVG(?score) AS ?averageScore) (SUM(?n) AS ?samples) WHERE {
  ?worker owf:usesOffering/owf:offersRelease <https://example.org/releases/exact-release> ;
          owf:hasHarness ?harness .
  ?observation owf:measuresWorker ?worker ; owf:metric <https://example.org/metrics/pass-rate> ;
               owf:rawScore ?score ; owf:scoreUnit ?unit ; owf:sampleSize ?n .
}
GROUP BY ?harness
```

## 6. Which estimates are stale or derived from expired offerings?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

SELECT DISTINCT ?estimate ?observedAt ?offeringEnd WHERE {
  ?estimate owf:derivedFromEvidence ?evidence ; owf:abilityForWorker/owf:usesOffering ?offering .
  ?evidence owf:observedAt ?observedAt .
  OPTIONAL { ?offering owf:validUntil ?offeringEnd }
  FILTER (?observedAt < NOW() - "P90D"^^xsd:duration
          || (BOUND(?offeringEnd) && ?offeringEnd <= NOW()))
}
```

## 7. What was known when a routing decision was made?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?decision ?snapshot ?digest ?policy ?time WHERE {
  ?decision a owf:RoutingDecision ; owf:usesSnapshot ?snapshot ;
            owf:policyVersion ?policy ; owf:decisionAt ?time .
  ?snapshot owf:snapshotDigest ?digest .
}
```

## 8. Did predicted success agree with verified outcomes?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?worker (AVG(?predicted) AS ?meanPrediction)
       (AVG(IF(?accepted, 1, 0)) AS ?acceptanceRate) (COUNT(*) AS ?outcomes) WHERE {
  ?decision owf:selectedWorker ?worker ; owf:hasCandidateQuote ?quote .
  ?quote owf:candidateWorker ?worker ; owf:successLowerBound ?predicted .
  ?outcome owf:outcomeForDecision ?decision ; owf:accepted ?accepted .
}
GROUP BY ?worker
```

## 9. Can a public snapshot be proven free of private predicates?

This is a release gate as well as a query: the result must be empty, then the snapshot must pass `owf:PublicExportPrivacyShape`.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>

SELECT ?subject ?predicate WHERE {
  {
    VALUES ?predicate {
      owf:promptText
      owf:repositoryUri
      owf:tenantId
      owf:credentialReference
      owf:rawArtifact
      owf:taskHasClass
      owf:taskApplicationDomain
      owf:taskArtifactType
      owf:taskAcceptanceProfile
      owf:requiresSkill
      owf:requiresTool
      owf:hasBudget
      owf:requiredSkill
      owf:minimumQuality
      owf:cashLimitMicrounits
      owf:latencyLimitMilliseconds
      owf:decisionForTask
      owf:usesSnapshot
      owf:hasCandidateQuote
      owf:hasRejectedCandidate
      owf:candidateWorker
      owf:expectedAcceptedCostMicrounits
      owf:successLowerBound
      owf:rejectedWorker
      owf:rejectionCode
      owf:selectedWorker
      owf:checkerWorker
      owf:decisionVerificationPolicy
      owf:decisionAt
      owf:policyVersion
      owf:outcomeForDecision
      owf:outcomeForTask
      owf:outcomeWorker
      owf:outcomeCheckerWorker
      owf:hasUsageRecord
      owf:accepted
      owf:actualCostMicrounits
      owf:finishedAt
      owf:projectIdentifier
      owf:repositoryFingerprint
      owf:impactEstimateForProject
      owf:usesEnvironmentalProfile
      owf:estimateStatus
      owf:estimatedValueMicrounits
      owf:unknownReason
      owf:baselineForProject
      owf:baselinePolicyDigest
      owf:savingsForProject
      owf:comparesBaseline
      owf:actualImpactEstimate
      owf:counterfactualImpactEstimate
      owf:savedImpactMicrounits
      owf:counterfactualMethod
      owf:reportForProject
      owf:reportDigest
    }
    ?subject ?predicate ?value .
  }
  UNION
  {
    VALUES ?privateType {
      owf:TaskSpec owf:StructuredTaskSpec owf:SkillRequirement owf:Budget
      owf:RoutingDecision owf:CandidateQuote owf:RejectedCandidate
      owf:OutcomeEvent owf:UsageRecord owf:Project owf:ImpactEstimate
      owf:Baseline owf:SavingsEstimate owf:ProjectReport
    }
    ?subject a ?privateType .
    BIND (rdf:type AS ?predicate)
  }
}
```

## 10. Which immutable offering revision closed before its replacement opened?

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?current ?previous ?previousEnd ?currentStart WHERE {
  ?current a owf:ProviderOffering ;
           owf:supersedesOffering ?previous ;
           owf:validFrom ?currentStart .
  ?previous owf:validUntil ?previousEnd .
}
ORDER BY ?currentStart ?current
```

## 11. Which source-backed environmental factors apply to an exact offering?

The result keeps location-based and market-based CO2e, water consumption and
water withdrawal on separate rows. Consumers may compare rows only when the
functional unit, measurement boundary, lifecycle phase and units are
compatible.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX prov: <http://www.w3.org/ns/prov#>

SELECT ?profile ?offering ?functionalUnit ?boundary ?metric ?phase
       ?numerator ?denominator ?activityUnit ?resultUnit ?source WHERE {
  ?profile a owf:EnvironmentalProfile ;
           owf:environmentalProfileForOffering ?offering ;
           owf:functionalUnit ?functionalUnit ;
           owf:measurementBoundary ?boundary ;
           owf:hasImpactTerm ?term ;
           prov:wasDerivedFrom ?source .
  ?term owf:impactMetric ?metric ;
        owf:lifecyclePhase ?phase ;
        owf:factorNumerator ?numerator ;
        owf:factorDenominator ?denominator ;
        owf:factorActivityUnit ?activityUnit ;
        owf:factorResultUnit ?resultUnit .
}
ORDER BY ?offering ?profile ?metric ?phase
```

## 12. Are location-based and market-based carbon factors kept distinct?

This query deliberately returns one row per accounting method; callers must not
sum the rows.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?offering ?profile ?carbonMethod ?phase ?numerator ?denominator WHERE {
  VALUES ?carbonMethod { owf:Co2eLocationBased owf:Co2eMarketBased }
  ?profile a owf:EnvironmentalProfile ;
           owf:environmentalProfileForOffering ?offering ;
           owf:hasImpactTerm ?term .
  ?term owf:impactMetric ?carbonMethod ;
        owf:lifecyclePhase ?phase ;
        owf:factorNumerator ?numerator ;
        owf:factorDenominator ?denominator .
}
ORDER BY ?offering ?profile ?carbonMethod ?phase
```

## 13. Does any public factor use an invalid denominator?

This is a release gate. The result must be empty. A numerator of zero may be a
known measured zero; a denominator must always be greater than zero. Missing
evidence is represented by an absent impact term, never by a zero denominator.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?term ?denominator WHERE {
  ?term a owf:ImpactTerm ; owf:factorDenominator ?denominator .
  FILTER (?denominator <= 0)
}
```

## Planned private projection

The following vocabulary is a forward contract for a local read model. The
append-only SQL ledger remains authoritative, and the current Rust/store crates
do not yet materialize project, impact, baseline, savings or report RDF nodes.

## 14. What estimated impact did optimization avoid for a private project?

This query is local-only. A result is a counterfactual estimate, not a deduction
from the project's actual environmental inventory.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>

SELECT ?project ?baseline ?metric ?phase ?actualValue ?counterfactualValue
       ?savedValue ?method WHERE {
  ?saving a owf:SavingsEstimate ;
          owf:savingsForProject ?project ;
          owf:comparesBaseline ?baseline ;
          owf:actualImpactEstimate ?actual ;
          owf:counterfactualImpactEstimate ?counterfactual ;
          owf:savedImpactMicrounits ?savedValue ;
          owf:counterfactualMethod ?method .
  ?actual owf:impactMetric ?metric ;
          owf:lifecyclePhase ?phase ;
          owf:estimatedValueMicrounits ?actualValue .
  ?counterfactual owf:estimatedValueMicrounits ?counterfactualValue .
}
ORDER BY ?project ?metric ?phase
```

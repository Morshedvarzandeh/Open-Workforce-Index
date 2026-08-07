# Ontology competency questions

These questions define the minimum useful public knowledge-graph projection. The SQLite ledgers remain authoritative; queries run against an immutable RDF snapshot. Prefixes are omitted below only where repeated.

```sparql
PREFIX owf: <https://openworkforce.dev/ns#>
PREFIX prov: <http://www.w3.org/ns/prov#>
```

## 1. Which exact workers can serve a given skill now?

```sparql
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
SELECT ?worker ?release ?harness (GROUP_CONCAT(STR(?tool); separator=", ") AS ?tools) WHERE {
  ?worker a owf:WorkerProfile ; owf:usesOffering/owf:offersRelease ?release ; owf:hasHarness ?harness .
  OPTIONAL { ?worker owf:supportsTool ?tool }
}
GROUP BY ?worker ?release ?harness
```

## 3. Which evidence supports an ability estimate?

```sparql
SELECT ?estimate ?worker ?skill ?mean ?evidence ?source WHERE {
  ?estimate a owf:AbilityEstimate ;
            owf:abilityForWorker ?worker ; owf:abilitySkill ?skill ;
            owf:estimateMean ?mean ; owf:derivedFromEvidence ?evidence .
  ?evidence prov:wasDerivedFrom ?source .
}
```

## 4. How much does harness choice change observed performance for one release?

```sparql
SELECT ?harness (AVG(?score) AS ?averageScore) (SUM(?n) AS ?samples) WHERE {
  ?worker owf:usesOffering/owf:offersRelease <https://example.org/releases/exact-release> ;
          owf:hasHarness ?harness .
  ?observation owf:measuresWorker ?worker ; owf:metric <https://example.org/metrics/pass-rate> ;
               owf:rawScore ?score ; owf:scoreUnit ?unit ; owf:sampleSize ?n .
}
GROUP BY ?harness
```

## 5. Which estimates are stale or derived from expired offerings?

```sparql
SELECT DISTINCT ?estimate ?observedAt ?offeringEnd WHERE {
  ?estimate owf:derivedFromEvidence ?evidence ; owf:abilityForWorker/owf:usesOffering ?offering .
  ?evidence owf:observedAt ?observedAt .
  OPTIONAL { ?offering owf:validUntil ?offeringEnd }
  FILTER (?observedAt < NOW() - "P90D"^^<http://www.w3.org/2001/XMLSchema#duration>
          || (BOUND(?offeringEnd) && ?offeringEnd <= NOW()))
}
```

## 6. What was known when a routing decision was made?

```sparql
SELECT ?decision ?snapshot ?digest ?policy ?time WHERE {
  ?decision a owf:RoutingDecision ; owf:usesSnapshot ?snapshot ;
            owf:policyVersion ?policy ; owf:decisionAt ?time .
  ?snapshot owf:snapshotDigest ?digest .
}
```

## 7. Did predicted success agree with verified outcomes?

```sparql
SELECT ?worker (AVG(?predicted) AS ?meanPrediction)
       (AVG(IF(?accepted, 1, 0)) AS ?acceptanceRate) (COUNT(*) AS ?outcomes) WHERE {
  ?decision owf:selectedWorker ?worker ; owf:hasCandidateQuote ?quote .
  ?quote owf:candidateWorker ?worker ; owf:successLowerBound ?predicted .
  ?outcome owf:outcomeForDecision ?decision ; owf:accepted ?accepted .
}
GROUP BY ?worker
```

## 8. Can a public snapshot be proven free of private predicates?

This is a release gate as well as a query: the result must be empty, then the snapshot must pass `owf:PublicExportPrivacyShape`.

```sparql
SELECT ?subject ?predicate WHERE {
  VALUES ?predicate {
    owf:promptText owf:repositoryUri owf:tenantId owf:credentialReference owf:rawArtifact
  }
  ?subject ?predicate ?value .
}
```

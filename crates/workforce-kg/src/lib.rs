//! Public, rebuildable RDF projection for workforce catalog data.
//!
//! This crate intentionally has no dependency on the private local store. That
//! makes it impossible for an RDF export to accidentally serialize a prompt,
//! repository path, credential reference, or private outcome.

use anyhow::{Context, Result};
use oxigraph::{io::RdfFormat, store::Store};

/// The versioned public ontology shipped with this release.
pub const CORE_ONTOLOGY: &str = include_str!("../../../ontology/open-workforce.ttl");

/// The SHACL ingestion and export contract shipped with this release.
pub const CORE_SHAPES: &str = include_str!("../../../ontology/open-workforce.shacl.ttl");

/// An in-memory semantic read model.
///
/// Operational records remain in SQLite. A graph is rebuilt from a public
/// snapshot so there is only one source of truth and no dual-write failure.
pub struct PublicGraph {
    store: Store,
}

impl PublicGraph {
    /// Creates an empty in-memory graph.
    pub fn new() -> Result<Self> {
        Ok(Self {
            store: Store::new().context("create in-memory RDF store")?,
        })
    }

    /// Parses trusted, bundled Turtle into the default graph atomically.
    ///
    /// This stays private: public callers cannot inject arbitrary RDF and then
    /// route it through the export surface. Catalog projection will accept only
    /// the typed public-index DTO in a later adapter.
    fn load_turtle(&self, turtle: &str) -> Result<()> {
        self.store
            .load_from_slice(RdfFormat::Turtle, turtle)
            .context("parse Turtle into public graph")
    }

    /// Loads the OWI ontology. SHACL shapes are parsed separately by
    /// [`validate_builtin_rdf`] because shapes are a validation contract, not
    /// catalog facts.
    pub fn with_builtin_ontology() -> Result<Self> {
        let graph = Self::new()?;
        graph.load_turtle(CORE_ONTOLOGY)?;
        Ok(graph)
    }

    /// Returns the number of RDF statements in this projection.
    pub fn statement_count(&self) -> Result<usize> {
        self.store.len().context("count public RDF statements")
    }

    /// Exports line-sorted N-Quads suitable for reviewable diffs.
    ///
    /// This is not RDF Dataset Canonicalization; snapshot signing will use a
    /// dedicated canonicalization step before it is enabled.
    pub fn sorted_nquads(&self) -> Result<String> {
        let bytes = self
            .store
            .dump_to_writer(RdfFormat::NQuads, Vec::new())
            .context("serialize public RDF graph")?;
        let serialized = String::from_utf8(bytes).context("N-Quads must be UTF-8")?;
        let mut lines: Vec<&str> = serialized.lines().collect();
        lines.sort_unstable();
        let mut output = lines.join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        Ok(output)
    }
}

/// Parses both built-in Turtle resources and returns their statement counts.
///
/// Full SHACL execution is deliberately behind a future validation adapter;
/// this v0.1 gate guarantees that both contracts are syntactically valid RDF.
pub fn validate_builtin_rdf() -> Result<(usize, usize)> {
    let ontology = PublicGraph::with_builtin_ontology()?;
    let shapes = PublicGraph::new()?;
    shapes.load_turtle(CORE_SHAPES)?;
    Ok((ontology.statement_count()?, shapes.statement_count()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_valid_turtle() -> Result<()> {
        let (ontology_count, shape_count) = validate_builtin_rdf()?;
        assert!(ontology_count > 20);
        assert!(shape_count > 20);
        Ok(())
    }

    #[test]
    fn nquads_export_is_sorted() -> Result<()> {
        let graph = PublicGraph::with_builtin_ontology()?;
        let output = graph.sorted_nquads()?;
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines.windows(2).all(|pair| pair[0] <= pair[1]));
        Ok(())
    }
}

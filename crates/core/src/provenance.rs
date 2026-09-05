//! Where a value came from, and how much we trust it.
//!
//! Every extracted field carries provenance so that (a) a human correction is never
//! clobbered by automation, (b) the UI can show confidence, and (c) two sources disagreeing
//! is a recordable fact rather than a coin flip.

use serde::{Deserialize, Serialize};

/// How a field's value was obtained. The ordinal is the **precedence order** used when
/// merging extraction stages: a lower-precedence stage never overwrites a higher one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Guessed from other fields (e.g. seniority from a title). Lowest trust.
    Inferred,
    /// Deterministic heuristics over generic markup.
    Rules,
    /// A language model read the posting.
    Llm,
    /// A site-specific adapter read known markup.
    Adapter,
    /// schema.org microdata / RDFa.
    Microdata,
    /// schema.org `JobPosting` JSON-LD.
    Jsonld,
    /// The site's own structured API (Greenhouse, Lever, ...).
    Api,
    /// A human typed it. Never overwritten by automation.
    Manual,
}

impl Provenance {
    /// Default confidence for a stage, used when the stage has nothing better to say.
    pub fn baseline_confidence(self) -> f32 {
        match self {
            Provenance::Manual => 1.0,
            Provenance::Api => 0.97,
            Provenance::Jsonld => 0.90,
            Provenance::Microdata => 0.80,
            Provenance::Adapter => 0.85,
            Provenance::Llm => 0.75,
            Provenance::Rules => 0.60,
            Provenance::Inferred => 0.45,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Provenance::Manual => "manual",
            Provenance::Api => "api",
            Provenance::Jsonld => "jsonld",
            Provenance::Microdata => "microdata",
            Provenance::Adapter => "adapter",
            Provenance::Llm => "llm",
            Provenance::Rules => "rules",
            Provenance::Inferred => "inferred",
        }
    }
}

impl std::str::FromStr for Provenance {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        Ok(match s {
            "manual" => Provenance::Manual,
            "api" => Provenance::Api,
            "jsonld" => Provenance::Jsonld,
            "microdata" => Provenance::Microdata,
            "adapter" => Provenance::Adapter,
            "llm" => Provenance::Llm,
            "rules" => Provenance::Rules,
            "inferred" => Provenance::Inferred,
            other => return Err(crate::Error::BadRequest(format!("unknown provenance: {other}"))),
        })
    }
}

/// A confidence in `[0, 1]`, clamped on construction so no downstream code has to check.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Confidence(f32);

impl Confidence {
    pub const CERTAIN: Confidence = Confidence(1.0);

    pub fn new(v: f32) -> Self {
        Confidence(if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) })
    }

    pub fn get(self) -> f32 {
        self.0
    }

    /// Is this low enough that the UI should prompt for a human check?
    pub fn is_low(self) -> bool {
        self.0 < 0.6
    }
}

/// A value plus the story of where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sourced<T> {
    pub value: T,
    pub provenance: Provenance,
    pub confidence: Confidence,
}

impl<T> Sourced<T> {
    pub fn new(value: T, provenance: Provenance) -> Self {
        Self {
            value,
            provenance,
            confidence: Confidence::new(provenance.baseline_confidence()),
        }
    }

    pub fn with_confidence(value: T, provenance: Provenance, confidence: f32) -> Self {
        Self {
            value,
            provenance,
            confidence: Confidence::new(confidence),
        }
    }

    pub fn manual(value: T) -> Self {
        Self {
            value,
            provenance: Provenance::Manual,
            confidence: Confidence::CERTAIN,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Sourced<U> {
        Sourced {
            value: f(self.value),
            provenance: self.provenance,
            confidence: self.confidence,
        }
    }

    /// Would `candidate` win against this value?
    pub fn outranked_by(&self, candidate: Provenance, candidate_confidence: f32) -> bool {
        if self.provenance == Provenance::Manual {
            return false; // FR-E-03: human edits are sticky.
        }
        match candidate.cmp(&self.provenance) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Equal => candidate_confidence > self.confidence.get(),
            std::cmp::Ordering::Less => false,
        }
    }
}

/// Fill `slot` with `candidate` only if the candidate has stronger provenance.
///
/// This is the whole of the merge rule described in `docs/03-architecture.md`; keeping it in
/// one function is what keeps stage ordering from becoming a pile of special cases.
pub fn merge_field<T>(slot: &mut Option<Sourced<T>>, candidate: Sourced<T>) {
    match slot {
        None => *slot = Some(candidate),
        Some(existing) => {
            if existing.outranked_by(candidate.provenance, candidate.confidence.get()) {
                *slot = Some(candidate);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_is_ordered_low_to_high() {
        assert!(Provenance::Manual > Provenance::Api);
        assert!(Provenance::Api > Provenance::Jsonld);
        assert!(Provenance::Jsonld > Provenance::Adapter);
        assert!(Provenance::Adapter > Provenance::Llm);
        assert!(Provenance::Llm > Provenance::Rules);
        assert!(Provenance::Rules > Provenance::Inferred);
    }

    #[test]
    fn manual_values_are_never_overwritten() {
        let mut slot = Some(Sourced::manual("Staff Engineer".to_string()));
        merge_field(&mut slot, Sourced::new("Senior Engineer".to_string(), Provenance::Api));
        assert_eq!(slot.unwrap().value, "Staff Engineer");
    }

    #[test]
    fn higher_precedence_wins_and_lower_loses() {
        let mut slot = Some(Sourced::new("llm".to_string(), Provenance::Llm));
        merge_field(&mut slot, Sourced::new("api".to_string(), Provenance::Api));
        assert_eq!(slot.as_ref().unwrap().value, "api");

        merge_field(&mut slot, Sourced::new("rules".to_string(), Provenance::Rules));
        assert_eq!(slot.unwrap().value, "api");
    }

    #[test]
    fn equal_precedence_breaks_ties_on_confidence() {
        let mut slot = Some(Sourced::with_confidence("a".to_string(), Provenance::Llm, 0.5));
        merge_field(
            &mut slot,
            Sourced::with_confidence("b".to_string(), Provenance::Llm, 0.9),
        );
        assert_eq!(slot.unwrap().value, "b");
    }

    #[test]
    fn confidence_is_clamped() {
        assert_eq!(Confidence::new(5.0).get(), 1.0);
        assert_eq!(Confidence::new(-1.0).get(), 0.0);
        assert_eq!(Confidence::new(f32::NAN).get(), 0.0);
    }
}

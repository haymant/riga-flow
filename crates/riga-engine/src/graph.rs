//! Generic tensor-value place/transition runtime for composing engine stages.
//!
//! The production Phoenix reference kernel remains conventional and fast; this graph is an
//! execution/orchestration layer, not the implementation of its inner path loop.
use riga_core::{RigaError, TraceLevel, TransitionStats};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum PlaceValue {
    F64(Vec<f64>),
    F32(Vec<f32>),
    Bool(Vec<bool>),
    U64(Vec<u64>),
}

impl PlaceValue {
    pub fn len(&self) -> usize {
        match self {
            Self::F64(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::U64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub sequence: u64,
    pub transition_id: String,
    pub input_count: u64,
    pub output_count: u64,
}

/// A pure stage: named immutable transition and its input/output places.
pub trait Transition: Send + Sync {
    fn id(&self) -> &str;
    fn inputs(&self) -> &[String];
    fn output(&self) -> &str;
    fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError>;
}

#[derive(Default)]
pub struct ExecutionGraph {
    source_places: BTreeMap<String, PlaceValue>,
    transitions: Vec<Box<dyn Transition>>,
}

impl ExecutionGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(
        &mut self,
        place: impl Into<String>,
        value: PlaceValue,
    ) -> Result<(), RigaError> {
        let name = place.into();
        if name.trim().is_empty() {
            return Err(RigaError::new(
                "invalid_place",
                "place name must not be empty",
            ));
        }
        if self.source_places.insert(name.clone(), value).is_some() {
            return Err(RigaError::new(
                "duplicate_place",
                format!("source place {name} already exists"),
            ));
        }
        Ok(())
    }

    pub fn add_transition(
        &mut self,
        transition: impl Transition + 'static,
    ) -> Result<(), RigaError> {
        if transition.id().trim().is_empty() || transition.output().trim().is_empty() {
            return Err(RigaError::new(
                "invalid_transition",
                "transition id and output place are required",
            ));
        }
        if self.transitions.iter().any(|t| t.id() == transition.id()) {
            return Err(RigaError::new(
                "duplicate_transition",
                format!("transition {} already exists", transition.id()),
            ));
        }
        if self.source_places.contains_key(transition.output())
            || self
                .transitions
                .iter()
                .any(|t| t.output() == transition.output())
        {
            return Err(RigaError::new(
                "duplicate_place",
                format!("output place {} has multiple writers", transition.output()),
            ));
        }
        self.transitions.push(Box::new(transition));
        Ok(())
    }

    /// Execute transitions in registration order. Each input must be a source or a prior output.
    /// Statistics are always returned; per-transition events are only materialized above Off.
    pub fn execute(&self, trace: TraceLevel) -> Result<ExecutionReport, RigaError> {
        let mut places = self.source_places.clone();
        let mut stats = Vec::with_capacity(self.transitions.len());
        let mut events = Vec::new();
        let mut produced = self.source_places.keys().cloned().collect::<BTreeSet<_>>();
        for (sequence, transition) in self.transitions.iter().enumerate() {
            let mut inputs = Vec::with_capacity(transition.inputs().len());
            let mut input_count = 0usize;
            for name in transition.inputs() {
                if !produced.contains(name) {
                    return Err(RigaError::new(
                        "graph_dependency",
                        format!(
                            "transition {} reads unavailable place {name}",
                            transition.id()
                        ),
                    ));
                }
                let value = places.get(name).ok_or_else(|| {
                    RigaError::new("graph_dependency", format!("place {name} has no value"))
                })?;
                input_count = input_count.max(value.len());
                inputs.push(value);
            }
            let output_value = transition.execute(&inputs)?;
            let output_count = output_value.len();
            places.insert(transition.output().to_owned(), output_value);
            produced.insert(transition.output().to_owned());
            stats.push(TransitionStats {
                transition_id: transition.id().to_owned(),
                input_count: input_count as u64,
                output_count: output_count as u64,
                probability: if input_count == 0 {
                    0.0
                } else {
                    output_count as f64 / input_count as f64
                },
                mean_payoff: 0.0,
                variance_payoff: 0.0,
                mean_delta: 0.0,
                mean_vega: 0.0,
            });
            if trace != TraceLevel::Off {
                events.push(ExecutionEvent {
                    sequence: sequence as u64,
                    transition_id: transition.id().to_owned(),
                    input_count: input_count as u64,
                    output_count: output_count as u64,
                });
            }
        }
        Ok(ExecutionReport {
            places,
            stats,
            events,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub places: BTreeMap<String, PlaceValue>,
    pub stats: Vec<TransitionStats>,
    pub events: Vec<ExecutionEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FilterPositive {
        inputs: Vec<String>,
    }
    impl Transition for FilterPositive {
        fn id(&self) -> &str {
            "positive_filter"
        }
        fn inputs(&self) -> &[String] {
            &self.inputs
        }
        fn output(&self) -> &str {
            "positive"
        }
        fn execute(&self, inputs: &[&PlaceValue]) -> Result<PlaceValue, RigaError> {
            let PlaceValue::F64(values) = inputs[0] else {
                return Err(RigaError::new("type_mismatch", "expected f64"));
            };
            Ok(PlaceValue::F64(
                values.iter().copied().filter(|x| *x > 0.0).collect(),
            ))
        }
    }

    #[test]
    fn graph_executes_pure_stages_and_keeps_stats_when_trace_is_off() {
        let mut graph = ExecutionGraph::new();
        graph
            .add_source("payoff", PlaceValue::F64(vec![-1.0, 0.0, 2.0]))
            .unwrap();
        graph
            .add_transition(FilterPositive {
                inputs: vec!["payoff".into()],
            })
            .unwrap();
        let report = graph.execute(TraceLevel::Off).unwrap();
        assert_eq!(report.places["positive"], PlaceValue::F64(vec![2.0]));
        assert_eq!(report.stats[0].input_count, 3);
        assert_eq!(report.stats[0].output_count, 1);
        assert!(report.events.is_empty());
    }

    #[test]
    fn graph_rejects_a_read_before_its_producer() {
        let mut graph = ExecutionGraph::new();
        graph
            .add_transition(FilterPositive {
                inputs: vec!["missing".into()],
            })
            .unwrap();
        let error = graph.execute(TraceLevel::Aggregated).unwrap_err();
        assert_eq!(error.code, "graph_dependency");
    }

    #[test]
    fn graph_emits_events_only_when_requested() {
        let mut graph = ExecutionGraph::new();
        graph
            .add_source("payoff", PlaceValue::F64(vec![1.0]))
            .unwrap();
        graph
            .add_transition(FilterPositive {
                inputs: vec!["payoff".into()],
            })
            .unwrap();
        assert_eq!(graph.execute(TraceLevel::Sampled).unwrap().events.len(), 1);
    }
}

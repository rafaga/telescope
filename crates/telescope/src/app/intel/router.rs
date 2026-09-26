//! Routing stage: turns a [`DetectionBatch`] into the set of output queues
//! that should fire.
//!
//! Every enabled [`OutputRule`] whose channel filter matches evaluates its
//! [`Condition`] against the batch's tags. Per kind, an action fires when at
//! least one `Emit` rule matches and no `Suppress` rule does ("stopping wins").

use std::collections::HashSet;
use webb::rules::{DetectionBatch, OutputKind, OutputMode, OutputRule, RulesConfig, Tag};

/// Evaluates the output rules over detection batches.
pub(crate) struct Router {
    outputs: Vec<OutputRule>,
}

impl Router {
    /// Builds a router from a rules configuration, keeping only the enabled
    /// output rules.
    pub(crate) fn new(rules: &RulesConfig) -> Self {
        Self {
            outputs: rules
                .outputs
                .iter()
                .filter(|rule| rule.enabled)
                .cloned()
                .collect(),
        }
    }

    /// The output kinds that fire for `batch`, in the fixed order
    /// visual, sound, log.
    pub(crate) fn actions(&self, channel: &str, batch: &DetectionBatch) -> Vec<OutputKind> {
        let tags: HashSet<Tag> = batch.tags();
        let mut emit = [false; 3];
        let mut suppress = [false; 3];
        for rule in &self.outputs {
            if !rule.channels.is_empty() && !rule.channels.iter().any(|name| name == channel) {
                continue;
            }
            if !rule.when.evaluate(&tags) {
                continue;
            }
            let index = kind_index(rule.kind);
            match rule.mode {
                OutputMode::Emit => emit[index] = true,
                OutputMode::Suppress => suppress[index] = true,
            }
        }
        let mut actions = Vec::new();
        for (index, kind) in [
            OutputKind::Visual,
            OutputKind::Sound,
            OutputKind::Log,
        ]
        .into_iter()
        .enumerate()
        {
            if emit[index] && !suppress[index] {
                actions.push(kind);
            }
        }
        actions
    }
}

fn kind_index(kind: OutputKind) -> usize {
    match kind {
        OutputKind::Visual => 0,
        OutputKind::Sound => 1,
        OutputKind::Log => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ops::Range;
    use webb::rules::{Condition, Detection, IntelLine};

    fn batch(tags: &[&str]) -> DetectionBatch {
        DetectionBatch {
            line: IntelLine {
                timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
                author: String::from("Pilot"),
                text: String::from("H-5GUI"),
            },
            detections: vec![Detection {
                rule_id: String::from("r"),
                captures: HashMap::new(),
                category: None,
                system_group: None,
                tags: tags.iter().map(|tag| tag.to_string()).collect(),
                span: Range { start: 0, end: 0 },
                matched: String::new(),
            }],
        }
    }

    fn rule(id: &str, kind: OutputKind, mode: OutputMode, when: Condition) -> OutputRule {
        OutputRule {
            id: id.to_string(),
            kind,
            mode,
            when,
            channels: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn an_emit_rule_fires_on_its_tags() {
        let rules = RulesConfig {
            inputs: Vec::new(),
            detections: Vec::new(),
            outputs: vec![rule(
                "log",
                OutputKind::Log,
                OutputMode::Emit,
                Condition::AllTrue {
                    tags: vec![String::from("ship")],
                },
            )],
        };
        let router = Router::new(&rules);
        assert_eq!(router.actions("intel", &batch(&["ship"])), [OutputKind::Log]);
        assert!(router.actions("intel", &batch(&["clear"])).is_empty());
    }

    #[test]
    fn a_suppress_rule_wins_over_an_emit_rule_of_the_same_kind() {
        let rules = RulesConfig {
            inputs: Vec::new(),
            detections: Vec::new(),
            outputs: vec![
                rule(
                    "emit",
                    OutputKind::Sound,
                    OutputMode::Emit,
                    Condition::AllTrue {
                        tags: vec![String::from("ship")],
                    },
                ),
                rule(
                    "suppress",
                    OutputKind::Sound,
                    OutputMode::Suppress,
                    Condition::AllTrue {
                        tags: vec![String::from("clear")],
                    },
                ),
            ],
        };
        let router = Router::new(&rules);
        assert_eq!(
            router.actions("intel", &batch(&["ship"])),
            [OutputKind::Sound]
        );
        assert!(router.actions("intel", &batch(&["ship", "clear"])).is_empty());
    }
}

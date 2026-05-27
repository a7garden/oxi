//! Declarative workflow DSL — YAML definitions that map to existing coordination APIs.
//!
//! WorkflowDefinition parses a YAML file describing a multi-step workflow.
//! The execution plan maps each step to the appropriate coordination module call:
//! - Parallel → AgentGroup::parallel()
//! - Chain → AgentGroup::sequential()
//! - ForEach → CoordinatedGroup::map_reduce()
//! - Vote → Consensus::start() + cast_vote()
//! - SetState → SharedMemory::write()

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A complete workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    /// Workflow name
    pub name: String,
    /// Human-readable description
    #[serde(default)]
    pub description: String,
    /// Ordered list of steps
    pub steps: Vec<WorkflowStepDef>,
}

/// A single step in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowStepDef {
    /// Run a single agent with a task.
    Run {
        agent: String,
        task: String,
        #[serde(default)]
        output: Option<String>,
    },
    /// Run multiple agents in parallel with the same task.
    Parallel {
        agents: Vec<String>,
        task: String,
        #[serde(default)]
        concurrency: Option<usize>,
    },
    /// Run agents sequentially, passing results forward.
    Chain {
        steps: Vec<WorkflowStepDef>,
    },
    /// Fan-out: run an agent for each item in a SharedMemory key.
    ForEach {
        items_key: String,
        #[serde(default)]
        namespace: Option<String>,
        agent: String,
        task_template: String,
        #[serde(default)]
        concurrency: Option<usize>,
    },
    /// Vote: ask multiple agents and aggregate by threshold.
    Vote {
        agents: Vec<String>,
        question: String,
        #[serde(default)]
        threshold: Option<f32>,
    },
    /// Set a value in SharedMemory.
    SetState {
        key: String,
        #[serde(default)]
        namespace: Option<String>,
        value: Value,
    },
}

impl WorkflowDefinition {
    /// Load a workflow definition from a YAML file.
    pub fn from_yaml_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read workflow file: {}", path))?;
        Self::from_yaml_str(&content)
    }

    /// Parse a workflow definition from a YAML string.
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let def: WorkflowDefinition = serde_yaml::from_str(yaml)
            .with_context(|| "Failed to parse workflow YAML")?;
        def.validate()?;
        Ok(def)
    }

    /// Validate the workflow definition.
    fn validate(&self) -> Result<()> {
        if self.name.is_empty() {
            anyhow::bail!("Workflow name must not be empty");
        }
        if self.steps.is_empty() {
            anyhow::bail!("Workflow must have at least one step");
        }
        // Recursively validate steps
        for (i, step) in self.steps.iter().enumerate() {
            Self::validate_step(step, i)?;
        }
        Ok(())
    }

    fn validate_step(step: &WorkflowStepDef, index: usize) -> Result<()> {
        match step {
            WorkflowStepDef::Run { agent, task, .. } => {
                if agent.is_empty() {
                    anyhow::bail!("Step {}: agent name must not be empty", index);
                }
                if task.is_empty() {
                    anyhow::bail!("Step {}: task must not be empty", index);
                }
            }
            WorkflowStepDef::Parallel {
                agents,
                task,
                concurrency,
                ..
            } => {
                if agents.is_empty() {
                    anyhow::bail!("Step {}: parallel must have at least one agent", index);
                }
                if task.is_empty() {
                    anyhow::bail!("Step {}: task must not be empty", index);
                }
                if let Some(c) = concurrency {
                    if *c == 0 {
                        anyhow::bail!("Step {}: concurrency must be > 0", index);
                    }
                }
            }
            WorkflowStepDef::Chain { steps } => {
                if steps.is_empty() {
                    anyhow::bail!("Step {}: chain must have at least one sub-step", index);
                }
                for (j, sub) in steps.iter().enumerate() {
                    Self::validate_step(sub, j)?;
                }
            }
            WorkflowStepDef::ForEach {
                agent,
                task_template,
                ..
            } => {
                if agent.is_empty() {
                    anyhow::bail!("Step {}: agent must not be empty", index);
                }
                if task_template.is_empty() {
                    anyhow::bail!("Step {}: task_template must not be empty", index);
                }
            }
            WorkflowStepDef::Vote {
                agents, question, ..
            } => {
                if agents.is_empty() {
                    anyhow::bail!("Step {}: vote must have at least one agent", index);
                }
                if question.is_empty() {
                    anyhow::bail!("Step {}: question must not be empty", index);
                }
            }
            WorkflowStepDef::SetState { key, .. } => {
                if key.is_empty() {
                    anyhow::bail!("Step {}: key must not be empty", index);
                }
            }
        }
        Ok(())
    }

    /// Count total steps (including nested).
    pub fn step_count(&self) -> usize {
        self.steps.iter().map(Self::count_step).sum()
    }

    fn count_step(step: &WorkflowStepDef) -> usize {
        match step {
            WorkflowStepDef::Chain { steps } => {
                1 + steps.iter().map(Self::count_step).sum::<usize>()
            }
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_workflow() {
        let yaml = r#"
name: code-review
steps:
  - type: run
    agent: reviewer
    task: "Review the code changes"
  - type: set_state
    key: review_result
    value: "pending"
"#;
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        assert_eq!(wf.name, "code-review");
        assert_eq!(wf.steps.len(), 2);
        assert_eq!(wf.step_count(), 2);
    }

    #[test]
    fn test_parse_parallel_workflow() {
        let yaml = r#"
name: parallel-test
steps:
  - type: parallel
    agents: [agent-a, agent-b, agent-c]
    task: "Analyze the codebase"
    concurrency: 2
"#;
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        assert_eq!(wf.name, "parallel-test");
        match &wf.steps[0] {
            WorkflowStepDef::Parallel {
                agents,
                concurrency,
                ..
            } => {
                assert_eq!(agents.len(), 3);
                assert_eq!(*concurrency, Some(2));
            }
            _ => panic!("Expected Parallel step"),
        }
    }

    #[test]
    fn test_parse_chain_workflow() {
        let yaml = r#"
name: pipeline
steps:
  - type: chain
    steps:
      - type: run
        agent: designer
        task: "Design the API"
      - type: run
        agent: implementer
        task: "Implement the design"
"#;
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        assert_eq!(wf.step_count(), 3); // 1 chain + 2 inner
    }

    #[test]
    fn test_parse_vote_workflow() {
        let yaml = r#"
name: consensus
steps:
  - type: vote
    agents: [agent-a, agent-b, agent-c]
    question: "Which approach is best?"
    threshold: 0.66
"#;
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        match &wf.steps[0] {
            WorkflowStepDef::Vote {
                agents,
                question,
                threshold,
            } => {
                assert_eq!(agents.len(), 3);
                assert_eq!(question, "Which approach is best?");
                assert_eq!(*threshold, Some(0.66));
            }
            _ => panic!("Expected Vote step"),
        }
    }

    #[test]
    fn test_validation_empty_name() {
        let yaml = r#"
name: ""
steps:
  - type: run
    agent: a
    task: t
"#;
        assert!(WorkflowDefinition::from_yaml_str(yaml).is_err());
    }

    #[test]
    fn test_validation_empty_steps() {
        let yaml = r#"
name: empty
steps: []
"#;
        assert!(WorkflowDefinition::from_yaml_str(yaml).is_err());
    }

    #[test]
    fn test_foreach_workflow() {
        let yaml = r#"
name: batch-process
steps:
  - type: for_each
    items_key: file_list
    agent: file-processor
    task_template: "Process file: {item}"
    concurrency: 4
"#;
        let wf = WorkflowDefinition::from_yaml_str(yaml).unwrap();
        match &wf.steps[0] {
            WorkflowStepDef::ForEach {
                items_key,
                agent,
                task_template,
                concurrency,
                ..
            } => {
                assert_eq!(items_key, "file_list");
                assert_eq!(agent, "file-processor");
                assert_eq!(task_template, "Process file: {item}");
                assert_eq!(*concurrency, Some(4));
            }
            _ => panic!("Expected ForEach step"),
        }
    }
}

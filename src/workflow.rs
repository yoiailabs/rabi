//! Workflow definitions and the validation needed for finite step transitions.
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, NOT_FOUND};

pub(crate) const DEFAULT_YAML: &str = include_str!("workflow/default.yaml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    Builtin,
    Repository,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Repository => "repository",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Entry {
    pub name: String,
    pub source: Source,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    Codex,
    Claude,
    Cursor,
}

impl Provider {
    /// Capability contract for validation; Claude remains unverified.
    pub fn enforces_read_only(self) -> bool {
        matches!(self, Self::Codex)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Agent {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AgentOverride {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Definition {
    pub schema_version: u32,
    pub name: String,
    pub version: u64,
    pub workspace: Workspace,
    pub agent: Option<Agent>,
    pub start: String,
    pub limits: Limits,
    #[serde(default)]
    pub loops: BTreeMap<String, i64>,
    pub steps: BTreeMap<String, Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Workspace {
    New,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Limits {
    pub max_steps: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Step {
    #[serde(default)]
    pub allow_dirty: bool,
    #[serde(flatten)]
    pub kind: StepKind,
    #[serde(rename = "on")]
    pub transitions: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepSummary {
    pub allow_dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum StepKind {
    Agent {
        mode: Mode,
        output: Output,
        #[serde(default)]
        context: Context,
        instruction: String,
        #[serde(default)]
        agent: AgentOverride,
    },
    Check {
        commands: Commands,
    },
    System {
        action: Action,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Mode {
    Edit,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Output {
    ReportV1,
    ReviewV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Context {
    Fresh(Fresh),
    Continue {
        #[serde(rename = "continue")]
        step: String,
    },
}

impl Default for Context {
    fn default() -> Self {
        Self::Fresh(Fresh::Fresh)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Fresh {
    Fresh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Commands {
    Cli(Cli),
    List(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Cli {
    Cli,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Action {
    Prepare,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Target {
    Step(String),
    Loop {
        to: String,
        #[serde(rename = "loop")]
        name: String,
    },
}

impl Target {
    fn step(&self) -> &str {
        match self {
            Self::Step(to) | Self::Loop { to, .. } => to,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Diagnostic {
    pub step: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
}

impl Diagnostic {
    fn new(step: Option<String>, message: impl Into<String>) -> Self {
        Self {
            step,
            line: None,
            column: None,
            message: message.into(),
        }
    }
}

pub(crate) struct Inspection {
    pub entry: Entry,
    pub yaml: String,
    pub definition: Option<Definition>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Inspection {
    pub fn valid(&self) -> bool {
        self.definition.is_some() && self.diagnostics.is_empty()
    }

    /// CLI overrides apply last to every resolved step. Changing provider drops
    /// model/effort inherited from another tool; model-only preserves providers.
    pub fn override_agents(
        &mut self,
        provider: Option<Provider>,
        model: Option<&str>,
    ) -> Result<()> {
        let Some(definition) = &mut self.definition else {
            return Ok(());
        };
        let resolved = definition.agents();
        for (id, step) in &mut definition.steps {
            if let StepKind::Agent { agent, .. } = &mut step.kind {
                let prior = resolved.get(id);
                let selected = provider.or_else(|| prior.map(|agent| agent.provider));
                let same = prior.is_some_and(|agent| Some(agent.provider) == selected);
                *agent = AgentOverride {
                    provider: selected,
                    model: model.map(str::to_owned).or_else(|| {
                        same.then(|| prior.and_then(|agent| agent.model.clone()))
                            .flatten()
                    }),
                    effort: same
                        .then(|| prior.and_then(|agent| agent.effort.clone()))
                        .flatten(),
                };
            }
        }
        definition.agent = None;
        self.diagnostics = definition.validate();
        self.yaml =
            serde_norway::to_string(definition).map_err(|error| Error::msg(error.to_string()))?;
        Ok(())
    }

    pub fn check_json(&self) -> serde_json::Value {
        serde_json::json!({"name":self.entry.name, "source":self.entry.source,
            "path":self.entry.path, "valid":self.valid(), "diagnostics":self.diagnostics})
    }
}

pub(crate) fn builtin() -> Entry {
    Entry {
        name: "default".into(),
        source: Source::Builtin,
        path: None,
    }
}

/// Discovery does not parse definitions: even invalid ones can be inspected.
pub(crate) fn list(checkout: &Path) -> Result<Vec<Entry>> {
    let mut entries = vec![builtin()];
    let dir = checkout.join(".rabi/workflows");
    let files = match fs::read_dir(&dir) {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(Error::msg(format!("Failed to list workflows: {error}"))),
    };
    let mut repository = Vec::new();
    for file in files {
        let file =
            file.map_err(|error| Error::msg(format!("Failed to list workflows: {error}")))?;
        let path = file.path();
        if path.extension().is_none_or(|extension| extension != "yaml") {
            continue;
        }
        if let Some(name) = path.file_stem() {
            repository.push(Entry {
                name: name.to_string_lossy().into_owned(),
                source: Source::Repository,
                path: Some(path),
            });
        }
    }
    repository.sort_by(|a, b| a.name.cmp(&b.name));
    entries.extend(repository);
    Ok(entries)
}

pub(crate) fn find(checkout: &Path, name: &str) -> Result<Entry> {
    list(checkout)?
        .into_iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| Error::status(NOT_FOUND, format!("Workflow '{name}' not found")))
}

impl Entry {
    pub fn inspect(self) -> Inspection {
        let mut diagnostics = Vec::new();
        if self.source == Source::Repository && self.name == "default" {
            diagnostics.push(Diagnostic::new(
                None,
                "The workflow name 'default' is reserved for the built-in definition",
            ));
        }
        let yaml = match &self.path {
            None => Ok(DEFAULT_YAML.to_owned()),
            Some(path) => fs::metadata(path).and_then(|metadata| {
                if metadata.is_file() {
                    fs::read_to_string(path)
                } else {
                    Err(std::io::Error::other(
                        "Workflow definition must be a regular file",
                    ))
                }
            }),
        };
        let (yaml, definition) = match yaml {
            Ok(yaml) => {
                let definition = match serde_norway::from_str::<Definition>(&yaml) {
                    Ok(definition) => {
                        diagnostics.extend(definition.validate());
                        Some(definition)
                    }
                    Err(error) => {
                        let location = error.location();
                        diagnostics.push(Diagnostic {
                            step: None,
                            line: location.as_ref().map(|l| l.line()),
                            column: location.as_ref().map(|l| l.column()),
                            message: format!("Invalid workflow YAML: {error}"),
                        });
                        None
                    }
                };
                (yaml, definition)
            }
            Err(error) => {
                diagnostics.push(Diagnostic::new(
                    None,
                    format!("Failed to read workflow: {error}"),
                ));
                (String::new(), None)
            }
        };
        Inspection {
            entry: self,
            yaml,
            definition,
            diagnostics,
        }
    }
}

impl Definition {
    fn resolve(&self, overrides: &AgentOverride) -> Option<Agent> {
        Some(Agent {
            provider: overrides
                .provider
                .or_else(|| self.agent.as_ref().map(|agent| agent.provider))?,
            model: overrides
                .model
                .clone()
                .or_else(|| self.agent.as_ref().and_then(|agent| agent.model.clone())),
            effort: overrides
                .effort
                .clone()
                .or_else(|| self.agent.as_ref().and_then(|agent| agent.effort.clone())),
        })
    }

    pub fn agents(&self) -> BTreeMap<String, Agent> {
        self.steps
            .iter()
            .filter_map(|(name, step)| {
                if let StepKind::Agent { agent, .. } = &step.kind {
                    self.resolve(agent).map(|agent| (name.clone(), agent))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn step_summaries(&self) -> BTreeMap<&str, StepSummary> {
        self.steps
            .iter()
            .map(|(name, step)| {
                (
                    name.as_str(),
                    StepSummary {
                        allow_dirty: step.allow_dirty,
                    },
                )
            })
            .collect()
    }

    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.schema_version != 1 {
            diagnostics.push(Diagnostic::new(
                None,
                "Unsupported workflow schema_version: expected 1",
            ));
        }
        if !self.steps.contains_key(&self.start) {
            diagnostics.push(Diagnostic::new(
                Some("start".into()),
                format!("Start step '{}' does not exist", self.start),
            ));
        }
        if self.limits.max_steps <= 0 {
            diagnostics.push(Diagnostic::new(
                Some("limits.max_steps".into()),
                "max_steps must be positive",
            ));
        }
        for (name, limit) in &self.loops {
            if *limit <= 0 {
                diagnostics.push(Diagnostic::new(
                    Some(format!("loops.{name}")),
                    "Loop limit must be positive",
                ));
            }
        }
        let agents = self.agents();
        for (name, step) in &self.steps {
            let path = format!("steps.{name}");
            let results: &[&str] = match &step.kind {
                StepKind::Agent {
                    output: Output::ReviewV1,
                    ..
                } => &["pass", "revise"],
                StepKind::Agent { .. } | StepKind::System { .. } => &["ok"],
                StepKind::Check { .. } => &["pass", "fail"],
            };
            for (result, target) in &step.transitions {
                let path = Some(format!("{path}.on.{result}"));
                if !results.contains(&result.as_str()) {
                    diagnostics.push(Diagnostic::new(
                        path.clone(),
                        format!("Invalid result '{result}' for this step type"),
                    ));
                }
                if !matches!(target.step(), "succeeded" | "stopped")
                    && !self.steps.contains_key(target.step())
                {
                    diagnostics.push(Diagnostic::new(
                        path.clone(),
                        format!("Transition target '{}' does not exist", target.step()),
                    ));
                }
                if let Target::Loop { name, .. } = target {
                    if !self.loops.contains_key(name) {
                        diagnostics.push(Diagnostic::new(path, format!("Undefined loop '{name}'")));
                    }
                }
            }
            if let StepKind::Agent {
                context,
                output,
                mode,
                ..
            } = &step.kind
            {
                if !agents.contains_key(name) {
                    diagnostics.push(Diagnostic::new(
                        Some(format!("{path}.agent.provider")),
                        "An agent provider is required",
                    ));
                }
                if matches!(mode, Mode::ReadOnly) {
                    if let Some(agent) = agents.get(name) {
                        if !agent.provider.enforces_read_only() {
                            diagnostics.push(Diagnostic::new(
                                Some(format!("{path}.mode")),
                                "Resolved provider cannot enforce read-only mode (only codex is currently verified)",
                            ));
                        }
                    }
                }
                if let Context::Continue { step: previous } = context {
                    if *output == Output::ReviewV1 {
                        diagnostics.push(Diagnostic::new(
                            Some(format!("{path}.context")),
                            "review-v1 steps must use context fresh",
                        ));
                    }
                    match self.steps.get(previous) {
                        Some(Step {
                            kind: StepKind::Agent { .. },
                            ..
                        }) => {
                            if let (Some(current), Some(prior)) =
                                (agents.get(name), agents.get(previous))
                            {
                                if current.provider != prior.provider {
                                    diagnostics.push(Diagnostic::new(
                                        Some(format!("{path}.context")),
                                        "Continue target must resolve to the same provider",
                                    ));
                                }
                            }
                        }
                        _ => diagnostics.push(Diagnostic::new(
                            Some(format!("{path}.context")),
                            format!("Continue target '{previous}' must exist and be an agent step"),
                        )),
                    }
                }
            }
        }
        // Kahn's algorithm considers every step, but only ordinary edges. No
        // reachability or unused-loop policy is imposed.
        let mut incoming: BTreeMap<&str, usize> =
            self.steps.keys().map(|name| (name.as_str(), 0)).collect();
        for step in self.steps.values() {
            for target in step.transitions.values() {
                if let Target::Step(to) = target {
                    if !matches!(to.as_str(), "succeeded" | "stopped") {
                        if let Some(count) = incoming.get_mut(to.as_str()) {
                            *count += 1;
                        }
                    }
                }
            }
        }
        let mut ready: VecDeque<&str> = incoming
            .iter()
            .filter_map(|(name, count)| (*count == 0).then_some(*name))
            .collect();
        let mut visited = 0;
        while let Some(name) = ready.pop_front() {
            visited += 1;
            if let Some(step) = self.steps.get(name) {
                for target in step.transitions.values() {
                    if let Target::Step(to) = target {
                        if !matches!(to.as_str(), "succeeded" | "stopped") {
                            if let Some(count) = incoming.get_mut(to.as_str()) {
                                *count -= 1;
                                if *count == 0 {
                                    ready.push_back(to);
                                }
                            }
                        }
                    }
                }
            }
        }
        if visited != self.steps.len() {
            diagnostics.push(Diagnostic::new(
                incoming
                    .iter()
                    .find_map(|(name, count)| (*count > 0).then(|| format!("steps.{name}.on"))),
                "Cycle without a loop edge",
            ));
        }
        diagnostics
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> Definition {
        serde_norway::from_str(DEFAULT_YAML).unwrap()
    }

    #[test]
    fn built_in_default_matches_the_plan_and_resolves_agent_defaults() {
        let inspected = builtin().inspect();
        assert!(inspected.valid(), "{:?}", inspected.diagnostics);
        let definition = inspected.definition.unwrap();
        assert_eq!(definition.version, 8);
        assert_eq!(definition.limits.max_steps, 15);
        assert_eq!(definition.loops["repair"], 2);
        assert_eq!(definition.steps.len(), 6);
        assert_eq!(definition.agents().len(), 4);
        for agent in definition.agents().values() {
            assert_eq!(agent.provider, Provider::Codex);
            assert_eq!(agent.model.as_deref(), Some("gpt-6.1-sol"));
            assert_eq!(agent.effort.as_deref(), Some("high"));
        }
    }

    #[test]
    fn validation_checks_only_transition_limits_and_context_contracts() {
        let valid = serde_json::to_value(definition()).unwrap();
        for (path, value, message) in [
            ("/start", serde_json::json!("missing"), "Start step"),
            (
                "/steps/prepare/on/ok",
                serde_json::json!("missing"),
                "Transition target",
            ),
            (
                "/steps/check/on/fail/loop",
                serde_json::json!("missing"),
                "Undefined loop",
            ),
            ("/limits/max_steps", serde_json::json!(0), "positive"),
            ("/loops/repair", serde_json::json!(-1), "positive"),
            (
                "/steps/review/context",
                serde_json::json!({"continue":"implement"}),
                "context fresh",
            ),
            (
                "/steps/fix/agent/provider",
                serde_json::json!("cursor"),
                "same provider",
            ),
            (
                "/steps/fix/context/continue",
                serde_json::json!("check"),
                "must exist and be an agent",
            ),
        ] {
            let mut value_map = valid.clone();
            *value_map.pointer_mut(path).unwrap() = value;
            let parsed: Definition = serde_json::from_value(value_map).unwrap();
            assert!(
                parsed
                    .validate()
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(message)),
                "{path}: {:?}",
                parsed.validate()
            );
        }
        let mut bounded = definition();
        bounded
            .steps
            .get_mut("implement")
            .unwrap()
            .transitions
            .insert(
                "ok".into(),
                Target::Loop {
                    to: "implement".into(),
                    name: "repair".into(),
                },
            );
        assert!(bounded.validate().is_empty());
        bounded
            .steps
            .get_mut("implement")
            .unwrap()
            .transitions
            .insert("ok".into(), Target::Step("implement".into()));
        assert!(bounded
            .validate()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Cycle without a loop edge")));
        let mut result = definition();
        result
            .steps
            .get_mut("check")
            .unwrap()
            .transitions
            .insert("ok".into(), Target::Step("stopped".into()));
        assert!(result
            .validate()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Invalid result")));
    }

    #[test]
    fn cli_overrides_apply_after_step_defaults_and_revalidate_capabilities() {
        let mut inspection = builtin().inspect();
        if let StepKind::Agent { agent, .. } = &mut inspection
            .definition
            .as_mut()
            .unwrap()
            .steps
            .get_mut("implement")
            .unwrap()
            .kind
        {
            agent.model = Some("step-model".into());
        }
        inspection
            .override_agents(Some(Provider::Codex), None)
            .unwrap();
        let agents = inspection.definition.as_ref().unwrap().agents();
        assert_eq!(agents["implement"].model.as_deref(), Some("step-model"));
        assert_eq!(agents["implement"].effort.as_deref(), Some("high"));
        inspection
            .override_agents(Some(Provider::Cursor), None)
            .unwrap();
        let agents = inspection.definition.as_ref().unwrap().agents();
        assert_eq!(agents["review"].provider, Provider::Cursor);
        assert_eq!(agents["review"].model, None);
        assert_eq!(agents["review"].effort, None);
        assert!(!inspection.valid());
        assert!(inspection
            .diagnostics
            .iter()
            .any(|d| d.message.contains("read-only")));
        inspection
            .override_agents(Some(Provider::Codex), Some("cli-model"))
            .unwrap();
        assert!(inspection.valid());
        let agents = inspection.definition.as_ref().unwrap().agents();
        assert!(agents
            .values()
            .all(|agent| agent.model.as_deref() == Some("cli-model") && agent.effort.is_none()));
        let snapshot: Definition = serde_norway::from_str(&inspection.yaml).unwrap();
        assert_eq!(snapshot.agents(), agents);
    }

    #[test]
    fn model_only_preserves_mixed_providers_and_continue_is_revalidated_after_override() {
        let mut inspection = builtin().inspect();
        if let StepKind::Agent { agent, .. } = &mut inspection
            .definition
            .as_mut()
            .unwrap()
            .steps
            .get_mut("implement")
            .unwrap()
            .kind
        {
            agent.provider = Some(Provider::Cursor);
        }
        inspection.override_agents(None, Some("cli-model")).unwrap();
        assert_eq!(
            inspection.definition.as_ref().unwrap().agents()["implement"].provider,
            Provider::Cursor
        );
        assert!(!inspection.valid()); // fix still continues Codex from Cursor
        inspection
            .override_agents(Some(Provider::Codex), None)
            .unwrap();
        assert!(inspection.valid());
        assert_eq!(
            inspection.definition.as_ref().unwrap().agents()["review"]
                .model
                .as_deref(),
            Some("cli-model")
        );
    }

    #[test]
    fn omitted_context_is_fresh_and_tool_specific_settings_are_not_restricted() {
        let yaml = DEFAULT_YAML
            .replace("    context: fresh\n", "")
            .replace("effort: high", "effort: arbitrary-tool-value");
        let mut value: serde_json::Value = serde_norway::from_str(&yaml).unwrap();
        value["unknown_key"] = serde_json::json!(true);
        value["steps"]["implement"]["unknown_key"] = serde_json::json!(true);
        value["steps"]["implement"]["agent"] =
            serde_json::json!({"model":"another-model","effort":"another-effort"});
        let definition: Definition = serde_json::from_value(value).unwrap();
        assert!(definition.validate().is_empty());
        assert_eq!(
            definition.agents()["implement"].model.as_deref(),
            Some("another-model")
        );
        assert_eq!(
            definition.agents()["implement"].effort.as_deref(),
            Some("another-effort")
        );
    }
}

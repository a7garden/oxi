//! List selection and wizard step types.

/// Rewind action choices for the rewind overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewindAction {
    RestoreBoth,
    RestoreConversation,
    RestoreCode,
    SummarizeFromHere,
    NeverMind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenAIServiceTierChoice {
    ProjectDefault,
    Flex,
    Priority,
}

/// Host-side action bound to a `/providers` row (`ProviderRow` →
/// `ProviderAction { provider, action }`).
///
/// Lives in `vtui-compat` (not `oxicode-cli`) because it is referenced
/// inside `InlineListSelection::ProviderAction`. Putting it here avoids
/// an import cycle: `oxicode-cli` already depends on `oxicode-vtui-compat`
/// but cannot introduce the reverse edge just for an enum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthAction {
    /// Prompt for a fresh API key (set / replace).
    SetApiKey,
    /// Kick off the OAuth `authorization_code` flow.
    StartOAuth,
    /// Remove the currently stored credential.
    RemoveKey,
}

/// Selection value returned from a list or wizard overlay.
///
/// The `Reasoning` variant carries a `String` reasoning-effort level rather
/// than a typed enum so that this type stays free of config-crate dependencies.
/// Callers convert to/from their local `ReasoningEffortLevel` as needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineListSelection {
    Model(usize),
    DynamicModel(usize),
    CustomProvider(usize),
    RefreshDynamicModels,
    Reasoning(String),
    DisableReasoning,
    OpenAIServiceTier(OpenAIServiceTierChoice),
    CustomModel,
    /// `/models` catalog browser — index into
    /// `RenderState::overlay_catalog_models`.
    CatalogModel(usize),
    /// `/providers` list — index into `RenderState::overlay_providers`.
    ProviderRow(usize),
    /// `/providers` → action-menu selection. Emitted by the per-provider
    /// action list overlay so the host can route the chosen `AuthAction`
    /// (set key / start OAuth / remove key) back to the auth pipeline.
    ProviderAction {
        provider: String,
        action: AuthAction,
    },
    Theme(String),
    Session(String),
    SessionForkMode {
        session_id: String,
        summarize: bool,
    },
    ConfigAction(String),
    SlashCommand(String),
    ToolApproval(bool),
    ToolApprovalDenyOnce,
    ToolApprovalSession,
    ToolApprovalPermanent,
    ToolApprovalEnable,
    FileConflictReload,
    FileConflictViewDiff,
    FileConflictAbort,
    SessionLimitIncrease(usize),
    RewindCheckpoint(usize),
    RewindAction(RewindAction),

    /// Selection shape used by legacy tabbed HITL flows.
    AskUserChoice {
        tab_id: String,
        choice_id: String,
        text: Option<String>,
    },

    /// Selection returned from the `request_user_input` HITL tool.
    RequestUserInputAnswer {
        question_id: String,
        selected: Vec<String>,
        other: Option<String>,
    },

    /// Plan confirmation dialog result (human-in-the-loop flow).
    PlanApprovalExecute,
    /// Return to planning to edit the plan file.
    PlanApprovalEditPlan,
    /// Return to planning to discuss and revise the plan in chat.
    PlanApprovalDiscuss,
    /// Auto-accept all future plans in this session.
    PlanApprovalAutoAccept,
    /// Hand off to the build primary agent and execute the plan.
    PlanApprovalSwitchBuild,
    /// Hand off to the auto primary agent (auto-execute with per-step HITL).
    PlanApprovalSwitchAuto,

    /// Settings-panel tab switch (`/settings` tabs).
    SettingsTab(usize),
    /// Settings-panel sidebar section jump.
    SettingsSection(usize),
    /// Settings-panel key-capture submenu targeting a named `SettingKey`.
    /// Carries the key's Debug name (a `String`) to avoid a dependency from
    /// `oxicode-vtui-compat` onto `oxicode-cli`'s `SettingKey` enum.
    SettingKeyCapture(String),
}

/// A selectable item inside a list overlay.
#[derive(Clone, Debug)]
pub struct InlineListItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub badge: Option<String>,
    pub indent: u8,
    pub selection: Option<InlineListSelection>,
    pub search_value: Option<String>,
}

/// A single step in a wizard modal flow.
#[derive(Clone, Debug)]
pub struct WizardStep {
    /// Title displayed in the tab header.
    pub title: String,
    /// Question or instruction shown above the list.
    pub question: String,
    /// Selectable items for this step.
    pub items: Vec<InlineListItem>,
    /// Whether this step has been completed.
    pub completed: bool,
    /// The selected answer for this step (if completed).
    pub answer: Option<InlineListSelection>,

    pub allow_freeform: bool,
    pub freeform_label: Option<String>,
    pub freeform_placeholder: Option<String>,
    pub freeform_default: Option<String>,
}

# TUI 슬래시 커맨드: `/model` (모델 선택) + `/key` (API 키 등록)

**날짜**: 2026-07-21
**상태**: 설계 (사용자 승인 대기)
**범위**: `oxi-pager` 크레이트 (신규 slash command 시스템 + KeyEntry/ModelPicker modal) + `oxi-cli` (TUI 실행 경로를 `xai-grok-pager` 외부 바이너리에서 `oxi-pager` 직접 호출로 전환, `pager_bridge.rs` 복원).
**버전 타겟**: v0.58
**선행 분석**:
- `oxi-ai/src/providers/register_builtins.rs` (minimax/minimax-cn builtin 검증됨)
- `oxi-ai/src/env_api_keys.rs:118-130` (ZAI_API_KEY, MINIMAX_API_KEY, MINIMAX_CN_API_KEY env 정의)
- `oxi-ai/src/model_registry.rs:82-86,742-` (ZAI/MiniMax 모델 카탈로그)
- `oxi-cli/src/store/auth_storage.rs:1039-1154` (AuthStorage가 SDK AuthProvider port로 이미 wire됨, sync fast-path 보유)
- `oxi-cli/src/setup_wizard.rs:216-280` (provider list, mask_key, model list 빌드 — 패턴 차용)
- `oxi-cli/src/app/agent_session.rs:694-719` (기존 `set_model(model_id)` 가 switch_model + save_last_used + session append 를 처리)
- `vendor/grok-build/crates/codegen/xai-grok-pager/src/slash/commands/model.rs` (참고용 패턴; submodule이므로 복사 X)

---

## 0. 동기

현재 `oxi` 의 대화형 TUI는 `xai-grok-pager` 외부 바이너리를 fork-exec로 실행한다
(`oxi-cli/src/bootstrap.rs:255-264`). 그 바이너리는
`vendor/grok-build/crates/codegen/xai-grok-pager` (submodule)의 결과물이다.

- 신규 사용자가 처음 `oxi` 를 실행하면 setup_wizard가 뜨지만, **실행 중** provider를 바꾸거나 API key를 등록하는 경로가 TUI 안에 없다.
- minimax, zai 같은 신규 provider는 카탈로그와 env key가 다 정의되어 있으나, TUI에서 한 번에 등록·전환할 수단이 없다.
- 사용자는 `~/.oxi/auth.json` 을 vi로 편집하거나 `oxi setup` 을 다시 실행해야 한다.

본 spec은 TUI 안에서 `/model` (provider/model picker) 과 `/key <provider>` (API key 등록) 두 slash command를 추가한다. oxi가 직접 소유한 `oxi-pager` 크레이트 안에서 구현하여 submodule 편집 없이 진행한다.

## 1. 비목표 (YAGNI)

- OAuth flow: API key 등록만 다룬다.
- Custom provider 추가 UI: 기존 `setup_wizard` 가 담당 (별개 spec).
- Fuzzy provider name match: v1은 exact match. 알 수 없는 이름은 builtins 목록을 보여주는 에러로 응답.
- Reasoning effort picker: 별개 기능.
- 키 마스킹 toggle (Ctrl+R reveal): v1은 항상 `mask_key()` 표시.
- 자동완성 dropdown (slash 입력 중 후보 순환): v2 후보. v1은 `<Tab>` 으로 완성.

## 2. 아키텍처

### 2.1 TUI 실행 경로 전환

```
[현재]
oxi-cli/src/bootstrap.rs::dispatch_run_mode
  → Command::new("xai-grok-pager").status()     (submodule 빌드물, fork-exec)

[목표]
oxi-cli/src/bootstrap.rs::dispatch_run_mode
  → pager_bridge::run_pager_with_agent(agent, auth, settings)
      → oxi_pager::run(user_tx, slash_tx, bg_rx)
          → handle_key + CommandRegistry::dispatch
```

`pager_bridge.rs` 는 `ddd1b171` 커밋에 존재했던 파일을 복원하고 확장한다:

1. `Arc<oxi_agent::Agent>` 를 받아서 background thread에서 `Agent::run_with_channel` 으로 구동.
2. `Arc<oxi_cli::store::auth_storage::AuthStorage>` 를 받아서 slash command 가 직접 호출할 수 있도록 pager에 주입.
3. `Arc<oxi_cli::App>` 를 받아서 `app.set_model(...)` 호출 + scrollback push 가능하도록 한다 (`App` 은 `AgentSession` 을 보유).
4. pager 측에 두 개의 채널을 노출: `user_tx: mpsc::Sender<String>` (agent 입력), `slash_tx: mpsc::Sender<SlashCmd>` (slash command 액션). 같은 background thread가 둘 다를 consume.

`Arc<ModelCatalog>` 은 pager가 직접 들고 있지 않다. Model picker의 모델 목록은 pager 시작 시점에 `oxi_ai::model_registry::get_all_models()` 의 정적 결과로 한 번 빌드하여 `PagerState.modal` 안의 `Vec<ModelEntry>` 로 들어간다 (5,000+ 항목이지만 string 복사 한 번).

### 2.2 Slash command 시스템 (oxi-pager 신규)

`oxi-pager/src/slash/` 디렉터리를 신설. 기존 `oxi-pager/src/slash.rs` (19-line stub)는 삭제하고 trait 정의를 위해 `slash/mod.rs` + `slash/command.rs` + `slash/commands/*.rs` 로 대체.

```rust
// oxi-pager/src/slash/command.rs
pub enum SlashCmd {
    /// Forward as plain prompt to the agent.
    SubmitToAgent(String),
    /// Open the API key entry modal for the named provider.
    OpenKeyEntry { provider: String },
    /// Open the model picker modal, optionally pre-selecting a provider.
    OpenModelPicker { initial_provider: Option<String> },
    /// Persist the entered API key for the given provider.
    SetApiKey { provider: String, key: String },
    /// Persist the chosen model as the session default.
    SetDefaultModel { provider: String, model_id: String },
    /// Surface an error message in the status line.
    ShowError(String),
}

pub trait SlashCommand: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn run(&self, args: &str) -> SlashCmd;
}

pub struct CommandRegistry { /* name/alias -> Arc<dyn SlashCommand> */ }
impl CommandRegistry {
    pub fn builtin() -> &'static Self { /* OnceLock<CommandRegistry> */ }
    pub fn dispatch(&self, text: &str) -> SlashCmd { /* /<cmd> <args> */ }
}
```

`CommandRegistry::builtin()` 은 정적 인스턴스 (`OnceLock<CommandRegistry>`) 로 캐시. 매 Enter마다 새로 만들지 않음.

**경계 (중요)**: `SlashCommand::run` 은 **순수 함수**다. `AuthStorage` / `Settings` / `App` 를 직접 호출하지 않고 `SlashCmd` enum 만 반환한다. 부수 효과는 bridge 의 `on_slash(cmd)` 핸들러가 수행한다. 이 경계 덕분에 `dispatch` 가 단위 테스트 가능 (`#[test]` 안에서 `AuthStorage::in_memory()` 같은 fake 없이).

`main_loop.rs::handle_key` 의 `KeyCode::Enter` 분기 수정:

```rust
KeyCode::Enter => {
    let text = std::mem::take(&mut s.prompt.text);
    s.prompt.cursor = 0;
    if text.starts_with('/') {
        let cmd = slash::CommandRegistry::builtin().dispatch(&text);
        let _ = slash_tx.send(cmd);  // bridge 가 consume
    } else {
        // Add to scrollback + forward to agent (현재 동작 유지)
        let _ = user_tx.send(text);
    }
}
```

### 2.3 `/key <provider>` 슬래시 커맨드

**파일**: `oxi-pager/src/slash/commands/key.rs` (신규)
**등록**: `oxi-pager/src/slash/commands/mod.rs` 의 `builtin_commands()` vec에 `Arc::new(KeyCommand)` 추가.

`KeyCommand::run(args: &str) -> SlashCmd` 동작 (순수):
1. `args.trim()` 으로 provider 이름 받음. 비어 있으면 `ShowError("Usage: /key <provider>")`.
2. `oxi_ai::register_builtins::get_builtin_provider(name)` 으로 검증. 없으면 `ShowError("Unknown provider: {name}. Available: anthropic, openai, google, zai, minimax, minimax-cn, ...")` (builtin 전체 이름을 join).
3. `SlashCmd::OpenKeyEntry { provider }` 반환.

**Modal**: `state::ModalKind::KeyEntry { provider: String, input: String }` 추가. 입력 중에는 `input` 이 누적, 화면에는 `*` 로 마스킹된 글자만 표시 (`mask_key(&input)`).

**모달 lifecycle (bridge 가 수행)**:
- bridge 가 `OpenKeyEntry { provider }` 수신 → `state.modal = Some(KeyEntry { provider, input: String::new() })`.
- 모달 활성 상태에서 키 입력은 `main_loop::handle_key` 의 모달 분기로 라우팅 (`Char` → input.push, `Backspace` → input.pop).
- 모달 활성 상태에서 Enter → `SlashCmd::SetApiKey { provider, key: input }` 으로 변환하여 다시 bridge 로 송신. `pager_bridge::on_slash(SetApiKey)` 이 `auth.set_api_key(&provider, key)` 호출. `state.modal = None`. scrollback에 `[system] API key saved for {provider}` 블록 push.
- 모달 활성 상태에서 Esc → `state.modal = None`.

### 2.4 `/model` 슬래시 커맨드

**파일**: `oxi-pager/src/slash/commands/model.rs` (신규)
**등록**: `oxi-pager/src/slash/commands/mod.rs` 에 `Arc::new(ModelCommand)` 추가.

`ModelCommand::run(args: &str) -> SlashCmd` 동작 (순수):
1. 인자 없이 호출: `SlashCmd::OpenModelPicker { initial_provider: None }`.
2. `/model anthropic` 같이 provider 이름 명시: `initial_provider: Some("anthropic")` 로 picker 열기. model 선택 시 provider는 고정.
3. 모르는 provider 이름이면 `/key` 와 동일한 에러 (`ShowError`).

**Modal**: `state::ModalKind::ModelPicker { providers: Vec<String>, selected_provider: usize, models: Vec<ModelEntry>, selected_model: usize, filter: String, focus: ModelPickerFocus }`.

`ModelEntry` 는 setup_wizard 의 `ModelEntry` 와 동일 형태 (`id`, `provider`, `context_window`). source는 `oxi_ai::model_registry::get_all_models()` (이미 존재, 정적 함수).

**레이아웃**: 2-pane. 왼쪽 pane = provider list (필터 입력 + 위/아래 키), 오른쪽 pane = 선택된 provider의 model list. 두 pane 모두 list cursor는 pager state의 `list_state: ListState` 를 재사용.

**모달 lifecycle (bridge 가 수행)**:
- bridge 가 `OpenModelPicker { initial_provider }` 수신 → `state.modal = Some(ModelPicker { providers: builtin list, selected_provider: index_of(initial_provider) or 0, models: filter all_models by initial_provider, selected_model: 0, filter: String::new(), focus: ModelPickerFocus::Provider })`.
- 모달 활성 상태에서 ↑/↓ → selected_provider 또는 selected_model 변경. provider 가 바뀌면 `models = filter by new provider`.
- 모달 활성 상태에서 Tab 또는 → → 모델 pane 으로 focus 이동. Shift+Tab 또는 ← → provider pane 으로 focus 이동.
- 모달 활성 상태에서 Enter (모델 pane focus) → `SlashCmd::SetDefaultModel { provider, model_id }`. bridge 가 `app.set_model(&format!("{provider}/{model_id}"))` 호출. 이 메서드는 내부적으로 `agent.switch_model()` + `settings.save_last_used()` + session append 를 처리한다 (`oxi-cli/src/app/agent_session.rs:694-719`). modal close. scrollback에 `[system] Default model: {provider}/{model_id}` push.
- 모달 활성 상태에서 Esc → modal close.

### 2.5 State 확장

`oxi-pager/src/state.rs` 의 `ModalKind` enum에 두 variant 추가:

```rust
pub enum ModalKind {
    // ... 기존 variants ...
    KeyEntry { provider: String, input: String },
    ModelPicker {
        providers: Vec<String>,
        selected_provider: usize,
        models: Vec<ModelEntry>,
        selected_model: usize,
        filter: String,
        focus: ModelPickerFocus,  // Provider | Model
    },
}

pub enum ModelPickerFocus { Provider, Model }
```

`PagerState` 는 그대로 — `state.modal: Option<ModalKind>` 가 이미 modal lifecycle을 관리한다.

### 2.6 Rendering

`oxi-pager/src/render/` (vendored grok render) 안의 `Block` + `Paragraph` + `List` 위젯 사용. 새 모달 두 개의 `render()` 분기를 추가:

- `KeyEntry`: 가운데 정렬된 박스 (가로 50%, 세로 5줄). 헤더: `Enter API key for {provider} (Esc to cancel)`. 본문: `*` 마스킹된 입력. footer: `Enter to save`.
- `ModelPicker`: 화면 상단 60% 영역의 두 pane. 좌 pane 헤더: `Providers`, 우 pane 헤더: `Models for {provider}`. 아래 footer: `Tab/←→ switch · ↑↓ navigate · Enter select · Esc cancel`.

기존 `render::theme::Theme` 그대로 사용. 새 색상 슬롯 불필요.

### 2.7 영속화

- API key: `oxi_cli::store::auth_storage::AuthStorage::set_api_key` (이미 디스크에 persist, `FileAuthStorage` 백엔드 사용). 변경 없음.
- Default model: `oxi_cli::store::settings::Settings::save_last_used` 는 `oxi-cli/src/app/agent_session.rs:694-719` 의 `set_model` 안에서 호출됨. 변경 없음.

스키마 마이그레이션 불필요.

## 3. 데이터 흐름

### 3.1 `/key zai` 실행 시

```
사용자:  /key zai<Enter>
  │
  ▼
oxi-pager::main_loop::handle_key(Enter)
  │ text = "/key zai"
  │ slash::CommandRegistry::builtin().dispatch("/key zai")
  │   → SlashCmd::OpenKeyEntry { provider: "zai" }
  ▼
slash_tx.send(OpenKeyEntry { provider: "zai" })
  │
  ▼ (pager_bridge 의 background thread)
pager_bridge::on_slash(OpenKeyEntry, &state, &auth, &app)
  │ state.modal = Some(ModalKind::KeyEntry { provider: "zai", input: "" })
  ▼
render::render() 가 KeyEntry 모달을 그림

사용자: sk-abc123<Enter>
  │
  ▼
handle_key(Enter) (modal 활성 상태)
  │ modal_key_dispatch → SlashCmd::SetApiKey { provider: "zai", key: "sk-abc123" }
  ▼
slash_tx.send(SetApiKey { .. })
  │
  ▼
pager_bridge::on_slash(SetApiKey)
  │ auth.set_api_key("zai", "sk-abc123")
  │ state.modal = None
  │ scrollback 에 "[system] API key saved for zai" 블록 push
  ▼
render::render() 가 modal 닫힌 본 화면 그림
```

### 3.2 `/model` 실행 시

```
사용자:  /model<Enter>
  │
  ▼
SlashCmd::OpenModelPicker { initial_provider: None }
  │
  ▼
state.modal = Some(ModelPicker { providers, models: vec![], .. })
  ▼
render::render() 가 좌 pane (provider list) 그림

사용자:  ↓↓↓<Enter> (anthropic 선택)
  │
  ▼
model picker 의 provider 변경 핸들러:
  │ models = all_models().filter(|m| m.provider == "anthropic")
  │ selected_model = 0
  ▼
render::render() 가 우 pane 갱신

사용자:  Tab ↓<Enter> (claude-3-5-sonnet 선택)
  │
  ▼
SlashCmd::SetDefaultModel { provider: "anthropic", model_id: "claude-3-5-sonnet" }
  │
  ▼
pager_bridge::on_slash(SetDefaultModel)
  │ app.set_model("anthropic/claude-3-5-sonnet")
  │   (내부: agent.switch_model() + settings.save_last_used() + session.append_model_change())
  │ state.modal = None
  │ scrollback 에 "[system] Default model: anthropic/claude-3-5-sonnet" push
  ▼
이후 agent 가 next prompt 부터 새 모델로 streaming
```

## 4. 파일 변경 요약

|파일|변경 종류|내용|
|---|---|---|
|`oxi-cli/src/pager_bridge.rs`|신규 (ddd1b171에서 복원 + 확장)|agent + auth + app를 pager에 주입, 두 채널 (user_tx, slash_tx) 관리, on_slash 핸들러에서 부수 효과 수행|
|`oxi-cli/src/bootstrap.rs`|수정|`xai-grok-pager` shell-out 제거, `pager_bridge::run_pager_with_agent` 호출로 교체|
|`oxi-cli/src/lib.rs`|수정|`pub mod pager_bridge;` 추가 (ddd1b171에 이미 있었음)|
|`oxi-cli/src/app/agent_session.rs`|수정|없음 — 기존 `set_model(model_id)` 사용 (`oxi-cli/src/app/agent_session.rs:694-719`)|
|`oxi-pager/src/slash.rs`|삭제|19-line stub 제거 (디렉터리 모듈로 대체)|
|`oxi-pager/src/slash/mod.rs`|신규|`CommandRegistry::builtin()` re-export, `OnceLock` 캐시|
|`oxi-pager/src/slash/command.rs`|신규|`SlashCmd` enum, `SlashCommand` trait, `CommandRegistry` 정의|
|`oxi-pager/src/slash/commands/mod.rs`|신규|`builtin_commands()` vec에 ModelCommand, KeyCommand 등록|
|`oxi-pager/src/slash/commands/key.rs`|신규|KeyCommand 구현 (순수)|
|`oxi-pager/src/slash/commands/model.rs`|신규|ModelCommand 구현 (순수)|
|`oxi-pager/src/state.rs`|수정|ModalKind에 KeyEntry, ModelPicker variant 추가, ModelPickerFocus enum 추가|
|`oxi-pager/src/main_loop.rs`|수정|Enter 분기에 slash 감지, modal 활성 상태 키 라우팅 (Char/Backspace/Enter/Esc/Tab/Arrows) 추가|
|`oxi-pager/src/reducer.rs`|수정|modal 활성 상태일 때의 키 처리, `OpenModal`/`CloseModal` action 추가 (PagerAction enum 확장)|
|`oxi-pager/src/render/mod.rs`|수정|KeyEntry, ModelPicker 모달 draw 분기 추가|
|`oxi-pager/Cargo.toml`|수정|신규 의존성 없음 (oxi-ai, oxi-agent 이미 있음)|

## 5. 테스트 전략

5단계 테스트 (각 단계는 이전 단계의 회귀를 감지):

1. **단위 (`oxi-pager/src/slash/commands/key.rs::tests`)**:
   - `KeyCommand.run("zai")` → `OpenKeyEntry { provider: "zai" }`.
   - `KeyCommand.run("")` → `ShowError("Usage: /key <provider>")`.
   - `KeyCommand.run("not-a-real-provider")` → `ShowError` 메시지에 "anthropic, openai, zai, minimax, ..." 포함.
   - `KeyCommand.run("minimax")` → `OpenKeyEntry { provider: "minimax" }` (minimax 가 first-class 인지 검증).
   - `KeyCommand.run("minimax-cn")` → `OpenKeyEntry { provider: "minimax-cn" }`.

2. **단위 (`oxi-pager/src/slash/commands/model.rs::tests`)**:
   - `ModelCommand.run("")` → `OpenModelPicker { initial_provider: None }`.
   - `ModelCommand.run("anthropic")` → `OpenModelPicker { initial_provider: Some("anthropic") }`.
   - `ModelCommand.run("not-a-real-provider")` → `ShowError` (provider 검증).

3. **Reducer (`oxi-pager/src/reducer.rs::tests`)**:
   - `reduce(Key('a'), state with modal=KeyEntry)`: input 에 "a" 추가, modal 유지.
   - `reduce(Key(Enter), state with modal=KeyEntry, input="sk-abc")`: action list 에 `CloseModal` 와 `NotifySlash(SlashCmd::SetApiKey { provider, key })` 포함.

4. **통합 (`oxi-cli/src/pager_bridge.rs::tests`)**:
   - `AuthStorage::in_memory()` 로 `on_slash(SetApiKey { provider: "zai", key: "sk-abc" })` 호출 → `auth.has("zai")` true, `auth.get_api_key("zai") == Some("sk-abc")`.
   - `App::default()` (또는 fake) 에 `on_slash(SetDefaultModel { provider: "anthropic", model_id: "claude-3-5-sonnet" })` 호출 → `app.last_used_model() == Some("anthropic/claude-3-5-sonnet")`.
   - `on_slash(OpenKeyEntry { provider: "zai" })` 호출 → pager_state 가 `Some(ModalKind::KeyEntry { provider: "zai", input: "" })` 로 전이.

5. **스모크 (수동)**:
   - `cargo run -p oxi` → 빈 환경에서 setup wizard 안 뜨는지 확인 (default model 이미 있으니).
   - TUI 안에서 `/key zai` 입력 → modal 뜸 → API key 입력 → 저장 → `[system] API key saved for zai` 표시.
   - 같은 session 에서 `/model` → picker → 모델 선택 → `[system] Default model: ...` 표시 → 다음 prompt 부터 새 모델로 응답.
   - non-slash 텍스트는 agent 로 정상 전달 (회귀 없음).
   - `cat ~/.oxi/auth.json` 에서 `zai` 키 확인.
   - `cat ~/.oxi/settings.toml` 에서 `last_used_model = "anthropic/claude-3-5-sonnet"` 확인.

각 테스트는 `#[cfg(test)] mod tests` 안에 inline, 기존 `auth_storage.rs` 와 `setup_wizard.rs` 의 test style을 따른다 (`assert_eq!` + `assert!(contains)`).

## 6. 위험과 완화

|위험|완화|
|---|---|
|`xai-grok-pager` 에서 `oxi-pager` 로 전환하면 시각적 폴리시가 회귀할 수 있다|vendored render (`oxi-pager/src/render/grok/`) 는 무수정. slash 감지는 `main_loop::handle_key` 한 곳만. non-slash 경로는 render 출력 byte-identical.|
|`oxi-pager` 가 451-line 스캐폴드라서 slash registry를 처음부터 짜야 한다|trait surface를 최소화 (1 method: `name`, 1 method: `run`). 양 command 합쳐 ~300 LOC. `SlashCommand::run` 을 순수 함수로 유지하여 단위 테스트 가능.|
|Background thread 가 `Arc<AuthStorage>` 와 `Arc<App>` 를 들고 있다가 lock 보유 중 panic|모든 `set_api_key` / `set_model` 호출은 `parking_lot::RwLock` short critical section 안에서 끝나므로 panic 없음. Mutex는 없으므로 `.await` 보유 위험도 없음.|
|신규 사용자가 `/key zai<Enter>` 만 치고 키 입력을 안 하면 `ShowError("Usage: /key <provider>")` 처럼 보여야 하는데 modal 이 뜨면 혼란|slash command parser는 인자 부족 시 `ShowError` 반환, modal을 열지 않는다. (parser는 `SlashCommand::run` 안에서 동기 처리.)|
|`/model` picker 에서 5,000+ 모델을 매번 모두 로드|초기 로드는 한 번만 (modal open 시점). provider 변경 시에만 우 pane의 model list를 filter한다. list 자체는 `Vec<ModelEntry>` (oxi-ai가 이미 보유).|
|Submodule `vendor/grok-build` 와의 contract|이번 spec은 vendor를 건드리지 않는다. `xai-grok-pager` 바이너리 의존이 사라지므로 `bootstrap.rs:255-264` 의 `if grok_pager.exists()` 가드도 제거한다.|
|streaming 중인 prompt 에 대해 `/model` 로 model 전환 시 in-flight stream 이 죽을 수 있다|기존 `set_model` (`oxi-cli/src/app/agent_session.rs:694-719`) 의 `agent.switch_model()` 이 streaming lock 을 잡고 swap. in-flight stream 은 abort. session append 에 `model_change` event 가 기록되어 next prompt 부터 새 모델이 사용됨. (회귀 위험 없음, 기존 동작과 동일.)|
|Test 4-2: `App::default()` 가 무거워서 unit test 에서 만들기 부담|테스트는 `App` 의 stub trait (예: `trait AppHandle { fn set_model(&self, ...) -> Result<()>; }`) 을 정의하고 pager_bridge 가 generic 으로 받게 하면 `App` 자체를 mock 가능. 또는 `set_model` 의 핵심 (settings save + last_used) 만 별도 trait 으로 분리.|

## 7. Acceptance Criteria

- [ ] `cargo build --workspace` 가 경고 0, 에러 0.
- [ ] `cargo clippy --workspace --exclude oxi-vendor-... -- -D warnings` 통과.
- [ ] `cargo nextest run -p oxi-pager` 통과.
- [ ] `cargo nextest run -p oxi-cli` 통과.
- [ ] TUI 안에서 `/key zai<Enter>` → 키 입력 modal → Enter 저장 → `[system] API key saved for zai` 표시.
- [ ] TUI 안에서 `/key<Enter>` → status line 에 "Usage: /key <provider>" 표시, modal 안 뜸.
- [ ] TUI 안에서 `/key notreal<Enter>` → status line 에 "Unknown provider: notreal. Available: anthropic, openai, google, ..." 표시.
- [ ] TUI 안에서 `/key minimax<Enter>` → modal 안의 헤더가 `Enter API key for minimax (Esc to cancel)`.
- [ ] TUI 안에서 `/model<Enter>` → 두 pane picker → provider 선택 → model 선택 → Enter → `[system] Default model: <provider>/<id>` 표시.
- [ ] TUI 안에서 non-slash 텍스트 입력 → agent 로 그대로 전달 (기존 동작 유지).
- [ ] 같은 session 에서 `/key` 로 등록한 키가 다음 prompt 부터 실제 LLM 호출에 사용됨.
- [ ] `oxi-cli/src/bootstrap.rs` 에 `xai-grok-pager` 문자열 0회 등장.
- [ ] `~/.oxi/auth.json` 에 `zai` 키가 저장됨 (smoke 후).
- [ ] `~/.oxi/settings.toml` 에 `last_used_model = "anthropic/claude-3-5-sonnet"` 가 저장됨 (smoke 후).

## 8. 후속 작업 (out of scope)

- `oxi-pager` 측에 slash command 자동완성 (위/아래 키로 후보 순환).
- `/key --remove <provider>` (저장된 키 삭제).
- `/model --show` (현재 default model + 사용 가능 모델 목록).
- OAuth flow (Anthropic / Google 등).
- Reasoning effort picker (`/effort`).
- `oxi-pager` 자체에 대한 통합 테스트 (PTY harness — vendor grok가 가진 1,500+ 테스트 패턴 차용).

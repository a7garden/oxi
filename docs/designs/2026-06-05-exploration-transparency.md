# Exploration Transparency — 검색·탐색 과정의 실시간 시각화

> **Status:** Design
> **Scope:** oxibrowser-core → oxi-agent → oxi-sdk → oxios-kernel → oxios-web
> **Precedes:** v0.13.0
> **Depends on:** v0.12 observability (shipped), per-tab routing (designed, pending)

---

## 1. 문제

### 1.1 사용자가 기대하는 것

Gemini나 Claude를 쓸 때, 웹 검색이 돌면 이런 식으로 보인다:

```
🔍 "Rust headless browser" 검색 중...
📋 검색 결과 10개 표시됨
📄 https://github.com/.../oxibrowser 열기...
📄 https://crates.io/crates/oxibrowser 열기...
✏️ 관련 정보 추출 중...
✅ 3개 사이트 탐색 완료 (12.4초)
```

각 단계가 **실시간으로** 카드에 표시된다. 사용자는 "아, 지금 이 사이트들을 뒤지고 있구나"를 즉시 알 수 있다.

### 1.2 현재의 한계

v0.12 관측성으로 페이지 수명주기 이벤트는 끝까지 간다:

```
NavigationStarted → "Opening https://example.com…"
DocumentReady     → "Loaded "Example" — 200 · 1.2 KB · 0 scripts · 245 ms"
```

하지만 **의미론적(sematic) 레벨**이 없다:

| 한계 | 설명 |
|------|------|
| **검색 의도를 모름** | `goto("https://google.com/search?q=rust")`는 그냥 "페이지 로딩"으로 보임. "검색 중"이 아님 |
| **탐색 경로가 안 보임** | A → B → C 순서로 방문해도 각각 독립 이벤트. "왜 B를 열었는지" 모름 |
| **추출 의도를 모름** | `extract --selector ".title"`은 CDP 커맨드일 뿐. "타이틀 추출 중"이 아님 |
| **다중 탭 연구가 안 보임** | 3개 탭이 병렬로 돌면 이벤트만 섞임. "무슨 조사를 하는 중인지" 알 수 없음 |
| **완료 요약이 없음** | 각 페이지의 DocumentReady는 있지만, "총 3페이지 탐색 완료" 같은 집계가 없음 |

### 1.3 핵심 인사이트

**페이지 수명주기는 브라우저 레벨, 탐색 의미는 에이전트 레벨이다.**

```
┌─────────────────────────────────────────────────┐
│ 브라우저 레벨 (oxibrowser)                       │
│   "이 URL 열었어. 200 OK. 12KB. 245ms."          │
│   → 사실 보고(factual). 의도(intent) 없음         │
├─────────────────────────────────────────────────┤
│ 에이전트 레벨 (oxi-agent)                         │
│   "Rust headless browser 검색결과에서             │
│    상위 3개 링크를 순차적으로 열어서               │
│    비교 정보를 추출하고 있어."                      │
│   → 의도 보고(intent). 왜 하는지 설명              │
└─────────────────────────────────────────────────┘
```

현재 v0.12는 브라우저 레벨만 있다. **에이전트 레벨의 탐색 의미론을 추가**해야 한다.

---

## 2. 설계 원칙

| # | 원칙 | 이유 |
|---|------|------|
| P1 | **브라우저는 사실만, 에이전트가 의미 부여** | oxibrowser는 검색엔진이 뭔지 모름. 의미는 oxi-agent가 생성 |
| P2 | **선언적 스텝, 명령형 틱** | "검색", "페이지 방문", "데이터 추출"은 스텝. 로딩 퍼센트 같은 틱은 없음 |
| P3 | **기존 BrowserEvent 재사용** | 새 이벤트 시스템을 만들지 않음. 브라우저 이벤트 위에 의미 레이어를 쌓음 |
| P4 | **UI는 구조만 받고 렌더링은 자유** | oxiOS WebUI가 카드 디자인을 결정. oxibrowser는 데이터만 보냄 |
| P5 | **스텝 트리, 플랫 리스트 아님** | "검색 → 결과 클릭 → 페이지 로드 → 추출"은 계층적 트리 |

---

## 3. 아키텍처

### 3.1 전체 파이프라인

```
┌──────────────────────────────────────────────────────────────┐
│ oxibrowser-core                                              │
│                                                              │
│  BrowserEvent (기존, 변경 없음)                               │
│    NavigationStarted, WaitingForSelector,                    │
│    DocumentReady, ScreenshotCaptured, NavigationFailed       │
│                                                              │
│  NavigationTrace (NEW)                                       │
│    리다이렉트 경로: [url1 → url2 → url3]                       │
│                                                              │
└───────────────────────┬──────────────────────────────────────┘
                        │ subscribe_events()
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ oxi-agent                                                    │
│                                                              │
│  ExplorationTracker (NEW)                                    │
│    BrowseTool의 고수준 액션을 탐색 스텝으로 변환               │
│                                                              │
│    BrowseTool.execute() 의 흐름:                              │
│      action="search"   → ExplorationStep::Searching          │
│      action="goto"     → ExplorationStep::VisitingPage       │
│      action="extract"  → ExplorationStep::ExtractingData     │
│      action="click"    → ExplorationStep::Interacting        │
│                                                              │
│    각 스텝은 하위 BrowserEvent를 수집해서 풍부하게 만듦         │
│                                                              │
│  AgentEvent::ExplorationProgress (NEW)                       │
│    { exploration_id, step, status }                           │
│                                                              │
└───────────────────────┬──────────────────────────────────────┘
                        │ AgentEvent stream
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ oxi-sdk                                                      │
│                                                              │
│  ExplorationEvent (re-export)                                │
│  ExplorationStep (re-export)                                 │
│                                                              │
│  OxiosSubscriber::on_exploration_progress()                  │
│                                                              │
└───────────────────────┬──────────────────────────────────────┘
                        │ KernelEvent
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ oxios-kernel                                                 │
│                                                              │
│  KernelEvent::ExplorationStep {                              │
│    session_id, exploration_id, parent_step_id?,              │
│    step: ExplorationStep, status: StepStatus                 │
│  }                                                           │
│                                                              │
│  → EventBus → SSE / WebSocket                               │
│                                                              │
└───────────────────────┬──────────────────────────────────────┘
                        │ WS chunk
                        ▼
┌──────────────────────────────────────────────────────────────┐
│ oxios-web                                                    │
│                                                              │
│  ExplorationTimeline 컴포넌트 (NEW)                           │
│    스텝 트리를 인디케이터 + 라벨로 렌더                        │
│    활성 스텝에 스피너                                         │
│    완료 스텝에 체크마크 + 소요시간                             │
│                                                              │
│  ActivityCard 확장                                           │
│    tool_call 카드 안에 미니 탐색 타임라인 임베드               │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### 3.2 데이터 모델

#### `ExplorationStep` — 탐색의 의미론적 단위

```rust
/// 에이전트 탐색의 의미론적 단계.
///
/// 브라우저 레벨의 BrowserEvent와 달리, 이것은 "무엇을 하려는지"를 나타냄.
/// 각 단계는 시작 → 완료/실패 수명주기를 가짐.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExplorationStep {
    /// 검색 엔진에 쿼리를 보냄
    Searching {
        query: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        engine: Option<String>,
    },

    /// 특정 URL의 페이지를 방문
    VisitingPage {
        url: String,
        /// 이 방문이 어떤 맥락에서 나왔는지 (검색 결과 클릭, 직접 입력 등)
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<VisitReason>,
    },

    /// 페이지에서 특정 데이터를 추출
    ExtractingData {
        /// 추출 대상 설명 ("제목과 본문", "가격 정보" 등)
        target: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },

    /// 페이지 내에서 상호작용 (클릭, 폼 제출, 스크롤 등)
    Interacting {
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
    },

    /// 분석/종합 단계 — 수집한 정보를 정리
    Synthesizing {
        sources_count: usize,
    },
}

/// 페이지 방문 이유
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisitReason {
    /// 검색 결과에서 클릭
    SearchResult { position: usize },
    /// 페이지 내 링크 클릭
    LinkFollowed { from_url: String },
    /// 에이전트가 직접 지정
    DirectNavigation,
    /// 리다이렉트
    Redirect,
}
```

#### `StepStatus` — 단계의 수명주기

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// 단계 시작
    Started,
    /// 진행 중 (하위 이벤트 수집 중)
    InProgress {
        /// 하위 BrowserEvent의 short_label
        detail: String,
    },
    /// 완료
    Completed {
        /// 완료시 요약 ("페이지 로드됨: 200 OK, 12KB")
        summary: String,
        /// 소요 시간 ms
        duration_ms: u64,
    },
    /// 실패
    Failed {
        error: String,
        duration_ms: u64,
    },
}
```

#### `ExplorationProgress` — 이벤트 버스에 실어 보내는 최종 이벤트

```rust
/// 탐색 진행 이벤트.
/// AgentEvent의 새 variant로 전송됨.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplorationProgress {
    /// 탐색 세션 고유 ID (BrowseTool execute 호출당 1개)
    pub exploration_id: String,
    /// 현재 단계의 ID (계층 구조 지원)
    pub step_id: String,
    /// 부모 단계 ID (검색 → 결과 방문 계층 등)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_step_id: Option<String>,
    /// 이 탐색의 최상위 목적 ("Rust headless browser 정보 수집")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// 단계 내용
    pub step: ExplorationStep,
    /// 단계 상태
    pub status: StepStatus,
    /// 연관된 탭 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<Uuid>,
}
```

---

## 4. oxibrowser-core 변경사항

### 4.1 변경 없음 (재확인)

v0.12의 `BrowserEvent` 4종 + `NavigationFailed` 1종은 **그대로** 사용한다.
새 이벤트를 추가하지 않는다. 이유:

1. 브라우저는 검색엔진, "탐색", "추출"의 개념을 모름
2. 의미는 전부 oxi-agent 레이어에서 부여
3. `NavigationStarted.url`에 `google.com/search?q=...`이 들어오면,
   agent가 그걸 해석해서 `ExplorationStep::Searching`으로 변환

### 4.2 Optional: 리다이렉트 추적

현재 `DocumentReady.final_url`은 리다이렉트 후 URL이지만,
**중간 경로**가 안 보인다. 이건 v0.13에서 추가할 수 있다:

```rust
// v0.13 후보 (지금 구현 안 함)
BrowserEvent::RedirectChain {
    tab_id: Uuid,
    chain: Vec<String>,  // ["http://a.com", "https://a.com", "https://www.a.com"]
}
```

**지금은 불필요.** `DocumentReady.final_url`만으로 충분하다.

---

## 5. oxi-agent 변경사항

### 5.1 ExplorationTracker

BrowseTool 내부에 탐색 상태를 관리하는 트래커를 추가한다.

```rust
// oxi-agent/src/tools/browse/exploration.rs (NEW)

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use parking_lot::Mutex;
use uuid::Uuid;

/// 탐색 단계 ID 생성기
static STEP_COUNTER: AtomicU32 = AtomicU32::new(1);

fn next_step_id() -> String {
    format!("step-{}", STEP_COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// 단일 탐색 세션의 상태.
/// BrowseTool::execute() 호출당 하나 생성됨.
pub struct ExplorationTracker {
    exploration_id: String,
    purpose: Option<String>,
    /// 에이전트 이벤트 emitter
    emitter: Arc<dyn Fn(ExplorationProgress) + Send + Sync>,
    /// 활성 단계 스택 (계층 구조)
    active_steps: Mutex<Vec<String>>,
}

impl ExplorationTracker {
    pub fn new(
        purpose: Option<String>,
        emitter: Arc<dyn Fn(ExplorationProgress) + Send + Sync>,
    ) -> Self {
        Self {
            exploration_id: format!("expl-{}", Uuid::new_v4().as_simple()),
            purpose,
            emitter,
            active_steps: Mutex::new(Vec::new()),
        }
    }

    /// 단계 시작
    pub fn begin_step(&self, step: ExplorationStep, tab_id: Option<Uuid>) -> StepGuard<'_> {
        let step_id = next_step_id();
        let parent_id = self.active_steps.lock().last().cloned();

        self.emit(ExplorationProgress {
            exploration_id: self.exploration_id.clone(),
            step_id: step_id.clone(),
            parent_step_id: parent_id.clone(),
            purpose: self.purpose.clone(),
            step,
            status: StepStatus::Started,
            tab_id,
        });

        self.active_steps.lock().push(step_id.clone());

        StepGuard {
            tracker: self,
            step_id,
            start: std::time::Instant::now(),
        }
    }

    /// 단계 진행 업데이트 (BrowserEvent로부터)
    pub fn update_step(&self, step_id: &str, detail: String, tab_id: Option<Uuid>) {
        // 현재 활성 단계인지 확인
        let active = self.active_steps.lock();
        if !active.contains(&step_id.to_string()) {
            return;
        }
        drop(active);

        self.emit(ExplorationProgress {
            exploration_id: self.exploration_id.clone(),
            step_id: step_id.to_string(),
            parent_step_id: None, // 업데이트에는 불필요
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(), // detail에 이미 있음
                reason: None,
            },
            status: StepStatus::InProgress { detail },
            tab_id,
        });
    }

    fn emit(&self, progress: ExplorationProgress) {
        (self.emitter)(progress);
    }

    fn complete_step(&self, step_id: &str, summary: String) {
        let mut active = self.active_steps.lock();
        active.retain(|s| s != step_id);

        self.emit(ExplorationProgress {
            exploration_id: self.exploration_id.clone(),
            step_id: step_id.to_string(),
            parent_step_id: None,
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(),
                reason: None,
            },
            status: StepStatus::Completed {
                summary,
                duration_ms: 0, // StepGuard에서 설정
            },
            tab_id: None,
        });
    }

    fn fail_step(&self, step_id: &str, error: String) {
        let mut active = self.active_steps.lock();
        active.retain(|s| s != step_id);

        self.emit(ExplorationProgress {
            exploration_id: self.exploration_id.clone(),
            step_id: step_id.to_string(),
            parent_step_id: None,
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(),
                reason: None,
            },
            status: StepStatus::Failed {
                error,
                duration_ms: 0,
            },
            tab_id: None,
        });
    }
}

/// RAII 가드: drop 시 단계 완료 처리
pub struct StepGuard<'a> {
    tracker: &'a ExplorationTracker,
    step_id: String,
    start: std::time::Instant,
}

impl<'a> StepGuard<'a> {
    /// 명시적 완료 (요약 포함)
    pub fn complete(self, summary: String) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        // tracker.complete_step 대신 직접 emit (duration 포함)
        let mut active = self.tracker.active_steps.lock();
        active.retain(|s| s != &self.step_id);
        drop(active);

        self.tracker.emit(ExplorationProgress {
            exploration_id: self.tracker.exploration_id.clone(),
            step_id: self.step_id.clone(),
            parent_step_id: None,
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(),
                reason: None,
            },
            status: StepStatus::Completed { summary, duration_ms },
            tab_id: None,
        });
        std::mem::forget(self); // Drop이 중복 emit하지 않도록
    }

    /// 명시적 실패
    pub fn fail(self, error: String) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let mut active = self.tracker.active_steps.lock();
        active.retain(|s| s != &self.step_id);
        drop(active);

        self.tracker.emit(ExplorationProgress {
            exploration_id: self.tracker.exploration_id.clone(),
            step_id: self.step_id.clone(),
            parent_step_id: None,
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(),
                reason: None,
            },
            status: StepStatus::Failed { error, duration_ms },
            tab_id: None,
        });
        std::mem::forget(self);
    }

    /// 진행 상태 업데이트
    pub fn update(&self, detail: String, tab_id: Option<Uuid>) {
        self.tracker.update_step(&self.step_id, detail, tab_id);
    }

    pub fn step_id(&self) -> &str { &self.step_id }
}

impl<'a> Drop for StepGuard<'a> {
    fn drop(&mut self) {
        // complete/fail이 호출되지 않은 경우 자동 완료
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let mut active = self.tracker.active_steps.lock();
        active.retain(|s| s != &self.step_id);
        drop(active);

        self.tracker.emit(ExplorationProgress {
            exploration_id: self.tracker.exploration_id.clone(),
            step_id: self.step_id.clone(),
            parent_step_id: None,
            purpose: None,
            step: ExplorationStep::VisitingPage {
                url: String::new(),
                reason: None,
            },
            status: StepStatus::Completed {
                summary: "completed".to_string(),
                duration_ms,
            },
            tab_id: None,
        });
    }
}
```

### 5.2 BrowseTool 통합

```rust
// oxi-agent/src/tools/browse/browse_tool.rs (수정)

impl BrowseTool {
    async fn execute(&self, ...) -> Result<AgentToolResult, ToolError> {
        // 1. 탐색 트래커 생성
        let exploration = ExplorationTracker::new(
            self.extract_purpose(&args),  // args에서 의도 추출
            self.emitter.clone(),
        );

        match action {
            "search" => {
                // 2. 검색 스텝 시작
                let step = exploration.begin_step(
                    ExplorationStep::Searching {
                        query: query.clone(),
                        engine: Some("google".to_string()),
                    },
                    None,
                );

                let result = tab.goto(&search_url).await?;
                step.complete(format!(
                    "검색 결과 로드됨: {}",
                    result.title
                ));

                // 3. 결과 추출 스텝
                let step = exploration.begin_step(
                    ExplorationStep::ExtractingData {
                        target: "검색 결과 링크".to_string(),
                        url: Some(result.url.clone()),
                    },
                    Some(tab.tab_id()),
                );
                let links = tab.query_all("a[href]").await?;
                step.complete(format!("{}개 결과 추출됨", links.len()));

                // 4. 상위 N개 페이지 방문
                for (i, url) in links.iter().take(3).enumerate() {
                    let step = exploration.begin_step(
                        ExplorationStep::VisitingPage {
                            url: url.clone(),
                            reason: Some(VisitReason::SearchResult {
                                position: i + 1,
                            }),
                        },
                        Some(tab.tab_id()),
                    );

                    let page = tab.goto(url).await?;
                    step.complete(format!(
                        "\"{}\" 로드 완료 — {}",
                        page.title, page.status
                    ));
                }
            }

            "browse" | "goto" => {
                let step = exploration.begin_step(
                    ExplorationStep::VisitingPage {
                        url: url.clone(),
                        reason: Some(VisitReason::DirectNavigation),
                    },
                    Some(tab.tab_id()),
                );

                let result = tab.goto(&url).await?;
                step.complete(format!(
                    "\"{}\" — {} · {}",
                    result.title,
                    result.status,
                    human_bytes(result.html.len() as u64),
                ));
            }

            "extract" => {
                let step = exploration.begin_step(
                    ExplorationStep::ExtractingData {
                        target: selector.clone().unwrap_or("전체 텍스트".to_string()),
                        url: Some(tab.content().await?.url),
                    },
                    Some(tab.tab_id()),
                );
                // ... 추출 로직 ...
                step.complete(format!("{} 바이트 추출됨", data.len()));
            }

            // click, fill 등
            _ => {
                let step = exploration.begin_step(
                    ExplorationStep::Interacting {
                        action: action.to_string(),
                        selector: selector.clone(),
                    },
                    Some(tab.tab_id()),
                );
                // ... 상호작용 로직 ...
                step.complete("완료".to_string());
            }
        }

        // 5. 종합 스텝 (선택적)
        if pages_visited > 1 {
            let step = exploration.begin_step(
                ExplorationStep::Synthesizing {
                    sources_count: pages_visited,
                },
                None,
            );
            step.complete(format!(
                "{}개 출처 탐색 완료",
                pages_visited,
            ));
        }
    }
}
```

### 5.3 AgentEvent 확장

```rust
// oxi-agent/src/events.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    // ... 기존 variants ...

    /// 탐색 진행 상황 (v0.28+)
    ExplorationProgress(ExplorationProgress),
}
```

### 5.4 BrowserEvent → InProgress 브릿지

기존 `on_progress` 콜백 경로와 새 `ExplorationProgress` 경로를 통합:

```rust
// BrowseTool의 내부 로직
// BrowserEvent가 들어오면 활성 스텝의 InProgress 업데이트로 변환

let tab_callback: Arc<dyn Fn(BrowserEvent) + Send + Sync> = {
    let exploration = exploration.clone();
    Arc::new(move |event: BrowserEvent| {
        // 1. 기존 ToolExecutionUpdate도 발생 (하위 호환)
        //    → agent loop의 기존 emit 로직

        // 2. 활성 탐색 스텝이 있으면 InProgress 업데이트
        if let Some(active_step_id) = exploration.active_step_id() {
            exploration.update_step(
                &active_step_id,
                event.short_label(),
                Some(event.tab_id()),
            );
        }
    })
};
tab.set_progress_callback(tab_callback);
```

---

## 6. oxi-sdk 변경사항

### 6.1 타입 재export

```rust
// oxi-sdk/src/lib.rs

#[cfg(feature = "native-browser")]
pub use oxi_agent::exploration::{
    ExplorationProgress,
    ExplorationStep,
    StepStatus,
    VisitReason,
};
```

### 6.2 Subscriber trait 확장 (선택적)

```rust
/// oxios가 구현하는 이벤트 구독 인터페이스
pub trait OxiosSubscriber {
    fn on_agent_event(&self, event: &AgentEvent);

    /// 탐색 진행 이벤트 (기본: no-op)
    fn on_exploration_progress(&self, progress: &ExplorationProgress) {
        // 기본 구현: AgentEvent로 래핑해서 on_agent_event로 전달
        self.on_agent_event(&AgentEvent::ExplorationProgress(progress.clone()));
    }
}
```

---

## 7. oxios-kernel 변경사항

### 7.1 KernelEvent 확장

```rust
// oxios-kernel/src/event_bus.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum KernelEvent {
    // ... 기존 variants ...

    /// 탐색 스텝 진행 상황
    ExplorationStep {
        session_id: String,
        /// 원본 ExplorationProgress를 그대로 전달
        progress: ExplorationProgress,
    },
}
```

### 7.2 agent_runtime.rs 매핑

```rust
AgentEvent::ExplorationProgress(progress) => {
    if let Some(ref sid) = transparency_session {
        let _ = kernel_handle.infra.publish(
            KernelEvent::ExplorationStep {
                session_id: sid.clone(),
                progress,
            },
        );
    }
}
```

### 7.3 SSE / WS chunk

```rust
// SSE
KernelEvent::ExplorationStep { session_id, progress } => serde_json::json!({
    "type": "exploration_step",
    "session_id": session_id,
    "exploration_id": progress.exploration_id,
    "step_id": progress.step_id,
    "parent_step_id": progress.parent_step_id,
    "purpose": progress.purpose,
    "step": progress.step,
    "status": progress.status,
    "tab_id": progress.tab_id,
}),

// WS chunk — 동일
```

---

## 8. oxios-web 변경사항

### 8.1 StreamChunk 타입 확장

```typescript
// types/index.ts
export type StreamChunk =
  | { type: 'tool_start'; ... }
  | { type: 'tool_progress'; ... }
  | { type: 'tool_end'; ... }
  | { type: 'exploration_step';      // ← NEW
      exploration_id: string;
      step_id: string;
      parent_step_id?: string;
      purpose?: string;
      step: ExplorationStep;
      status: StepStatus;
      tab_id?: string;
    }
  | ... ;
```

### 8.2 데이터 모델

```typescript
// types/exploration.ts (NEW)

export type ExplorationStep =
  | { type: 'searching'; query: string; engine?: string }
  | { type: 'visiting_page'; url: string; reason?: VisitReason }
  | { type: 'extracting_data'; target: string; url?: string }
  | { type: 'interacting'; action: string; selector?: string }
  | { type: 'synthesizing'; sources_count: number };

export type VisitReason =
  | { type: 'search_result'; position: number }
  | { type: 'link_followed'; from_url: string }
  | { type: 'direct_navigation' }
  | { type: 'redirect' };

export type StepStatus =
  | { type: 'started' }
  | { type: 'in_progress'; detail: string }
  | { type: 'completed'; summary: string; duration_ms: number }
  | { type: 'failed'; error: string; duration_ms: number };

export interface ExplorationNode {
  exploration_id: string;
  step_id: string;
  parent_step_id?: string;
  purpose?: string;
  step: ExplorationStep;
  status: StepStatus;
  tab_id?: string;
  children: ExplorationNode[];
}

// ExplorationNode 트리 빌더
export function buildExplorationTree(
  steps: ExplorationNode[]
): ExplorationNode[] {
  const byId = new Map<string, ExplorationNode>();
  const roots: ExplorationNode[] = [];

  for (const step of steps) {
    byId.set(step.step_id, { ...step, children: [] });
  }

  for (const step of steps) {
    const node = byId.get(step.step_id)!;
    if (step.parent_step_id && byId.has(step.parent_step_id)) {
      byId.get(step.parent_step_id)!.children.push(node);
    } else {
      roots.push(node);
    }
  }

  return roots;
}
```

### 8.3 ChatStore 확장

```typescript
// stores/chat.ts

// exploration_steps를 exploration_id별로 그룹화해서 보관
interface ChatActivity {
  // ... 기존 필드 ...
  exploration?: {
    id: string;
    purpose?: string;
    steps: ExplorationNode[];
    isRunning: boolean;
    startedAt: number;
  };
}

// chunkToActivity에 exploration_step 케이스 추가
case 'exploration_step': {
  const chunk = raw as ExplorationStepChunk;
  // 기존 tool_call 카드를 찾아서 exploration 데이터 갱신
  return {
    type: 'exploration_update',
    exploration_id: chunk.exploration_id,
    step_id: chunk.step_id,
    parent_step_id: chunk.parent_step_id,
    purpose: chunk.purpose,
    step: chunk.step,
    status: chunk.status,
    tab_id: chunk.tab_id,
  };
}
```

### 8.4 UI 컴포넌트

#### ExplorationTimeline

```tsx
// components/chat/exploration-timeline.tsx (NEW)

interface Props {
  steps: ExplorationNode[];
  isRunning: boolean;
}

export function ExplorationTimeline({ steps, isRunning }: Props) {
  return (
    <div className="space-y-1 ml-1">
      {steps.map((step) => (
        <StepRow key={step.step_id} step={step} isRunning={isRunning} />
      ))}
    </div>
  );
}

function StepRow({ step, isRunning }: { step: ExplorationNode; isRunning: boolean }) {
  const isActive = isRunning && step.status.type !== 'completed' && step.status.type !== 'failed';
  const isCompleted = step.status.type === 'completed';
  const isFailed = step.status.type === 'failed';

  return (
    <div className="flex items-start gap-2 text-xs">
      {/* 인디케이터 */}
      <div className="mt-0.5 shrink-0">
        {isActive && <Loader2 className="h-3 w-3 animate-spin text-blue-500" />}
        {isCompleted && <Check className="h-3 w-3 text-green-500" />}
        {isFailed && <X className="h-3 w-3 text-red-500" />}
        {!isActive && !isCompleted && !isFailed && (
          <Circle className="h-3 w-3 text-muted-foreground" />
        )}
      </div>

      {/* 라벨 */}
      <div className="flex-1 min-w-0">
        <span className={cn(
          "text-muted-foreground",
          isCompleted && "text-foreground",
          isFailed && "text-red-500",
        )}>
          {stepLabel(step.step)}
        </span>

        {/* 진행 중 세부사항 */}
        {step.status.type === 'in_progress' && (
          <span className="ml-1 text-muted-foreground">
            — {step.status.detail}
          </span>
        )}

        {/* 완료 요약 */}
        {step.status.type === 'completed' && step.status.summary && (
          <span className="ml-1 text-muted-foreground">
            — {step.status.summary}
          </span>
        )}

        {/* 소요 시간 */}
        {isCompleted && (
          <span className="ml-1 text-muted-foreground text-[10px]">
            {(step.status.duration_ms / 1000).toFixed(1)}s
          </span>
        )}

        {/* 자식 스텝 */}
        {step.children.length > 0 && (
          <div className="ml-3 mt-0.5 border-l border-border pl-2">
            <ExplorationTimeline steps={step.children} isRunning={isRunning} />
          </div>
        )}
      </div>
    </div>
  );
}

/** 스텝 타입 → 사람이 읽는 라벨 */
function stepLabel(step: ExplorationStep): string {
  switch (step.type) {
    case 'searching':
      return `🔍 "${step.query}" 검색`;
    case 'visiting_page':
      return `📄 ${shortenUrl(step.url)}`;
    case 'extracting_data':
      return `📋 ${step.target} 추출`;
    case 'interacting':
      return `👆 ${step.action}`;
    case 'synthesizing':
      return `✨ ${step.sources_count}개 출처 종합`;
  }
}

function shortenUrl(url: string): string {
  try {
    const u = new URL(url);
    const path = u.pathname === '/' ? '' : u.pathname.slice(0, 30);
    return u.hostname + path;
  } catch {
    return url.slice(0, 40);
  }
}
```

#### ActivityCard 통합

```tsx
// activity-card.tsx (수정)

// tool_call 카드 내부에 탐색 타임라인 임베드
{activity.exploration && (
  <div className="mt-2 pl-2 border-l-2 border-muted">
    <ExplorationTimeline
      steps={activity.exploration.steps}
      isRunning={activity.exploration.isRunning}
    />
    {activity.exploration.purpose && (
      <div className="mt-1 text-[10px] text-muted-foreground">
        목적: {activity.exploration.purpose}
      </div>
    )}
  </div>
)}
```

### 8.5 최종 UI 모습

```
┌─────────────────────────────────────────────────┐
│ 🔧 browse                                       │
│                                                  │
│  "Rust headless browser 비교 정보 수집"           │
│                                                  │
│  🔄 🔍 "rust headless browser" 검색 — 1.2s      │
│  ✅ 📄 google.com/search… — 45개 결과           │
│  ✅ 📄 github.com/oxibrowser — 200 · 2.1s       │
│  ✅ 📄 crates.io/crates/oxibrowser — 200 · 0.8s │
│  🔄 📄 docs.rs/oxibrowser — Loading…            │
│  ⏳ 📋 비교 정보 추출                             │
│  ⏳ ✨ 3개 출처 종합                              │
│                                                  │
│  경과: 8.4초                                     │
└─────────────────────────────────────────────────┘
```

---

## 9. 기존 ToolExecutionUpdate와의 관계

### 9.1 두 이벤트가 공존한다

```
BrowserEvent (저수준)
  ↓ ProgressCallback
AgentEvent::ToolExecutionUpdate { partial_result, tab_id }  ← 기존
  ↓ emitter
AgentEvent::ExplorationProgress { step, status }            ← 새로운
```

### 9.2 UI에서 어떻게 보이나

| 이벤트 | UI 표현 | 대상 |
|--------|---------|------|
| `ToolExecutionUpdate` | ActivityCard의 progress 텍스트 줄 | 단순 browse 액션 |
| `ExplorationProgress` | ActivityCard 내부의 미니 타임라인 | 복합 탐색 (search → visit → extract) |

### 9.3 하위 호환

- `ExplorationProgress`가 없는 구버전 oxi-agent는 기존 `ToolExecutionUpdate`만 보냄
- 구버전 oxios-web은 `exploration_step` chunk를 무시 (unknown type)
- 신버전은 둘 다 보고, UI가 풍부해짐

---

## 10. 구현 순서

### Phase 1 — oxi-agent 탐색 트래커 (3-4일)

| 작업 | 파일 | LoC |
|------|------|-----|
| `ExplorationStep`, `StepStatus`, `ExplorationProgress` 타입 | `oxi-agent/src/tools/browse/exploration.rs` (NEW) | ~150 |
| `ExplorationTracker` + `StepGuard` | 동일 | ~150 |
| `AgentEvent::ExplorationProgress` variant 추가 | `oxi-agent/src/events.rs` | ~5 |
| BrowseTool에 트래커 통합 | `oxi-agent/src/tools/browse/browse_tool.rs` | ~80 |
| BrowserEvent → InProgress 브릿지 | 동일 | ~20 |
| 단위 테스트 | `oxi-agent/src/tools/browse/exploration_tests.rs` (NEW) | ~100 |

### Phase 2 — oxi-sdk export (0.5일)

| 작업 | 파일 | LoC |
|------|------|-----|
| 탐색 타입 re-export | `oxi-sdk/src/lib.rs` | ~5 |
| 버전 bump | `oxi-sdk/Cargo.toml` | 1 |

### Phase 3 — oxios-kernel 이벤트 파이프라인 (1일)

| 작업 | 파일 | LoC |
|------|------|-----|
| `KernelEvent::ExplorationStep` variant | `oxios-kernel/src/event_bus.rs` | ~10 |
| agent_runtime 매핑 | `oxios-kernel/src/agent_runtime.rs` | ~15 |
| SSE/WS chunk 변환 | `oxios-web/src/routes/events.rs`, `chat.rs` | ~30 |
| 테스트 | 기존 rfc015_tests 확장 | ~30 |

### Phase 4 — oxios-web 프론트엔드 (2-3일)

| 작업 | 파일 | LoC |
|------|------|-----|
| TypeScript 타입 | `web/src/types/exploration.ts` (NEW) | ~60 |
| StreamChunk 확장 | `web/src/types/index.ts` | ~10 |
| chunkToActivity 확장 | `web/src/stores/chat.ts` | ~40 |
| ExplorationTimeline 컴포넌트 | `web/src/components/chat/exploration-timeline.tsx` (NEW) | ~120 |
| ActivityCard 통합 | `web/src/components/chat/activity-card.tsx` | ~20 |
| 테스트 | `web/src/__tests__/stores.test.ts` | ~40 |

**총: ~700 LoC, 7-9일**

---

## 11. 파일 변경 요약

| 프로젝트 | 파일 | 액션 |
|----------|------|------|
| **oxi-agent** | `src/tools/browse/exploration.rs` | **NEW** |
| | `src/tools/browse/exploration_tests.rs` | **NEW** |
| | `src/tools/browse/browse_tool.rs` | 수정 |
| | `src/tools/browse/mod.rs` | 수정 (mod 추가) |
| | `src/events.rs` | 수정 (variant 추가) |
| | `Cargo.toml` | 버전 bump |
| **oxi-sdk** | `src/lib.rs` | 수정 (re-export) |
| | `Cargo.toml` | 버전 bump |
| **oxios-kernel** | `src/event_bus.rs` | 수정 (variant 추가) |
| | `src/agent_runtime.rs` | 수정 (매핑 추가) |
| **oxios-web** | `src/routes/events.rs` | 수정 |
| | `src/routes/chat.rs` | 수정 |
| **oxios-web/frontend** | `web/src/types/exploration.ts` | **NEW** |
| | `web/src/types/index.ts` | 수정 |
| | `web/src/stores/chat.ts` | 수정 |
| | `web/src/components/chat/exploration-timeline.tsx` | **NEW** |
| | `web/src/components/chat/activity-card.tsx` | 수정 |
| | `web/src/__tests__/stores.test.ts` | 수정 |
| **oxibrowser** | *(변경 없음)* | — |

---

## 12. 리스크 분석

| 리스크 | 확률 | 영향 | 대응 |
|--------|------|------|------|
| 탐색 스텝이 너무 세분화되어 UI가 노이즈가 됨 | 중 | 중 | BrowseTool action별로 스텝 생성을 조절. search/visit/extract만. click/scroll은 생략 가능 |
| StepGuard의 Drop이 중복 emit | 낮음 | 낮음 | `mem::forget` 명시적 complete/fail 시. Drop은 안전망 |
| 다중 탭 병렬 탐색 시 스텝 순서 꼬임 | 중 | 낮음 | parent_step_id로 트리 구성. 순서가 아닌 계층으로 표시 |
| oxios-web이 기존 tool_progress와 exploration_step을 중복 표시 | 낮음 | 낮음 | exploration이 활성일 때 tool_progress는 숨기고 타임라인으로 대체 |
| 구버전 호환성 | 낮음 | 낮음 | `#[non_exhaustive]`, `#[serde(skip_serializing_if)]`, unknown chunk 무시 |

---

## 13. 열린 질문

1. **탐색 목적(purpose) 자동 추출**: BrowseTool args에서 어떻게 purpose를 뽑을까?
   - 옵션 A: LLM이 tool_call args에 `purpose` 필드를 명시적으로 넣음
   - 옵션 B: action + args 휴리스틱으로 유추 (search → query, goto → URL 기반)
   - 옵션 C: 항상 None, UI는 스텝들만 보여줌
   - **추천**: B로 시작, 필요하면 A로 전환

2. **스텝 세분화 수준**: 모든 action을 스텝으로 만들까?
   - 옵션 A: search, goto, extract만 (3종)
   - 옵션 B: click, fill, press 포함 (5종)
   - 옵션 C: wait_for, screenshot도 포함 (7종)
   - **추천**: A로 시작. UI 노이즈 최소화

3. **다중 탭 동시 탐색 표시**: 탭 2개가 병렬로 돌면 UI는?
   - 옵션 A: 단일 타임라인에 인터리브
   - 옵션 B: 탭별로 분리된 서브타임라인
   - **추천**: B. tab_id로 그룹핑

4. **지속형 탐색 (streaming)**: 에이전트가 탐색 결과를 스트리밍하면?
   - ExplorationProgress는 편의상 보내고, 완료 시 종합
   - oxios-web은 isRunning=true 동안 계속 업데이트
   - tool_end가 오면 isRunning=false + 최종 상태

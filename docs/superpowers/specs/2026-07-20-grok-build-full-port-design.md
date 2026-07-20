# grok-build 전체 TUI + Agent 런타임 이식

**날짜:** 2026-07-20
**상태:** 설계 승인 대기

## 1. 목표

grok-build(`xai-org/grok-build`)의 TUI(`xai-grok-pager`)와 agent 런타임을 oxi 워크스페이스로 verbatim vendoring.
oxi의 기존 TUI, agent, SDK를 grok 스택으로 완전히 교체한다. oxi-ai만 provider adapter로 유지.

## 2. 아키텍처

### 이식 후 구조

```
oxi/
├── oxi-ai/                          # 유지 — grok inference adapter로 연결
├── oxi-vendor-grok-pager/           # 🆕 TUI application (393K lines)
├── oxi-vendor-grok-shell/           # 🆕 CLI, session, config, tools
├── oxi-vendor-grok-agent/           # 🆕 Agent runtime
├── oxi-vendor-grok-pager-render/    # ✅ 기 vendored (fix 필요)
├── oxi-vendor-grok-markdown/        # ✅ 기 vendored
├── oxi-vendor-grok-markdown-core/   # ✅ 기 vendored
├── oxi-vendor-grok-mermaid/         # ✅ 기 vendored
├── oxi-vendor-ratatui-textarea/     # ✅ 기 vendored
├── oxi-vendor-ratatui-inline/       # ✅ 기 vendored
├── oxi-vendor-grok-paths/           # ✅ 기 vendored
├── oxi-vendor-tty-utils/            # ✅ 기 vendored
├── oxi-grok-bridge/                 # 🆕 oxi-ai → grok inference adapter (~500 lines)
└── ... ~40 more vendored crates     # 🆕 transitive deps
```

### 삭제 대상

| Crate | 이유 |
|---|---|
| `oxi-agent` | grok의 agent runtime으로 대체 |
| `oxi-sdk` | grok의 shell이 port system을 내장 |
| `oxi-tui` | grok pager로 완전 대체 |
| `oxi-pager` | grok pager로 완전 대체 |
| `oxi-cli/src/tui/` | grok pager로 대체 |
| `oxi-vendor-grok-shim` | verbatim vendoring으로 불필요 |
| `oxi-hashline` | grok-shell이 자체 diff/patch 포맷 사용 |
| `oxi-lsp` | grok-shell이 자체 LSP 통합 제공 |
| `oxi-mnemopi` | grok이 자체 memory 시스템 사용 |
| `oxi-snapcompact` | grok이 자체 compaction 제공 |
| `oxi-sandbox` | grok이 자체 sandbox 제공 |

### 유지 대상

| Component | 역할 |
|---|---|
| `oxi-ai` | LLM provider 추상화. `oxi-grok-bridge`가 grok sampling trait으로 변환 |
| `oxi-cli` (bootstrap, store) | composition root. grok pager 초기화 및 oxi-ai provider 등록 |
| `oxi-grok-bridge` | oxi-ai → grok inference adapter (~500 lines) |

### Composition Root (oxi-cli/src/main.rs)

```rust
// 개념 — grok pager를 oxi-ai provider로 초기화
fn main() {
    let registry = ProviderRegistry::with_builtins();  // oxi-ai
    let backend = OxiSamplingBackend::new(registry);    // oxi-grok-bridge
    let config = load_config();                         // oxi-cli/store
    xai_grok_pager::run(backend, config);               // grok pager
}
```

`oxi-cli/src/store/`는 grok-shell이 자체 config/session persistence를 가지므로
점진적으로 제거. 초기에는 oxi-cli가 config를 로드해 grok config 포맷으로 변환.

## 3. 의존성 트리

`cargo tree -p xai-grok-pager` 기준 ~50개 internal crate:

```
oxi-vendor-grok-pager
├── oxi-vendor-grok-pager-render ✅
├── oxi-vendor-grok-shell 🆕
│   ├── oxi-vendor-grok-shell-base 🆕
│   ├── oxi-vendor-grok-shell-session-support 🆕
│   ├── oxi-vendor-grok-shared 🆕
│   ├── oxi-vendor-grok-agent 🆕
│   ├── oxi-vendor-grok-tools 🆕
│   ├── oxi-vendor-grok-config 🆕
│   ├── oxi-vendor-grok-config-types 🆕
│   ├── oxi-vendor-grok-mcp 🆕
│   ├── oxi-vendor-grok-memory 🆕
│   ├── oxi-vendor-grok-auth 🆕
│   ├── oxi-vendor-grok-workspace 🆕
│   ├── oxi-vendor-grok-sandbox 🆕
│   └── ... transitive deps
├── oxi-vendor-grok-markdown ✅
├── oxi-vendor-grok-mermaid ✅
├── oxi-vendor-ratatui-textarea ✅
├── oxi-vendor-ratatui-inline ✅
├── oxi-vendor-grok-paths ✅
├── oxi-vendor-tty-utils ✅
└── oxi-grok-bridge 🆕 (inference adapter)
```

## 4. 통합 지점: Inference Adapter

grok의 agent는 `xai_grok_shell::sampling` 모듈을 통해 LLM 호출을 수행한다.
`oxi-grok-bridge`는 이 sampling trait을 oxi-ai의 `Provider::stream()`으로 구현한다:

```rust
// oxi-grok-bridge/src/lib.rs (개념)
impl grok::SamplingBackend for OxiSamplingBackend {
    async fn stream(&self, req: SamplingRequest) -> Result<Stream, Error> {
        let provider = self.registry.resolve(&req.model)?;
        let stream = provider.stream(&model, &context, opts).await?;
        // ProviderEvent → grok SamplingEvent 변환
        Ok(transform_stream(stream))
    }
}
```

**Bridge API surface (spike 측정):** 60개 타입/함수. 대부분 verbatim vendoring으로 해결.
Adapter가 직접 구현해야 하는 것은 inference trait 하나뿐.

## 5. ratatui 0.29 → 0.30 마이그레이션

Spike 결과: pager 자체는 ratatui 0.29→0.30에서 오류 없음.
21개 오류는 `xai-ratatui-inline`에서만 발생 — oxi는 이미 `B: Backend<Error = io::Error>` 바운드로 패치 완료.

## 6. 마이그레이션 단계

### Phase 1: 전체 vendoring
1. `cargo tree -p xai-grok-pager --prefix depth`로 전체 internal dependency 목록 추출
2. 각 crate를 `oxi-vendor-*`로 복사 (이름 규칙: `xai-grok-foo` → `oxi-vendor-grok-foo`)
3. Cargo.toml 일괄 변환 스크립트:
   - `name = "xai-*"` → `name = "oxi-vendor-*"`
   - `edition.workspace = true` → `edition = "2024"`
   - 내부 경로 참조: `xai-grok-foo = { path = "..." }` → `oxi-vendor-grok-foo = { path = "../oxi-vendor-grok-foo" }`
   - `workspace = true` → 개별 버전 명시 (grok workspace Cargo.toml에서 추출)
4. oxi workspace `members`에 모든 vendored crate 등록
5. `[workspace.dependencies]`에 누락된 external dep 추가
6. `cargo check --workspace` 성공까지 반복

### Phase 3: 기존 코드 제거
1. oxi-agent, oxi-sdk, oxi-tui, oxi-pager 제거
2. oxi-cli에서 미사용 import 정리
3. CI/CD 파이프라인 업데이트

### Phase 4: 검증
1. `cargo check --workspace`
2. `cargo test` (grok 테스트는 vendoring 범위에서 제외 가능)
3. TUI 실행 smoke test

## 7. 리스크

| Risk | Mitigation |
|---|---|
| oxi 기능 소실 (port system, issue system, memory) | grok이 동등 기능 제공. 필요시 추후 확장 |
| 50 crate vendoring으로 workspace 비대화 | git history에 영향 없음 (신규 파일). 컴파일 시간 증가는 LTO/profile로 상쇄 |
| grok 버전 업데이트 추적 어려움 | SOURCE_REV 파일로 vendor source commit 기록 |
| oxi-ai provider trait과 grok inference trait 불일치 | adapter에서 변환. 대부분 1:1 매핑 가능 |

## 8. 검증

- `cargo check --workspace` 통과
- `oxi-vendor-grok-pager` standalone compile
- oxi-cli binary에서 grok TUI 실행 smoke test

# oxi-cli 소스 코드 분석 보고서

**분석 대상:** `/Volumes/MERCURY/PROJECTS/oxi/oxi-cli/src/` (약 45,000줄, 78개 `.rs` 파일)  
**분석일:** 2026-05-14  
**분석자:** 자동화된 코드 리뷰 에이전트

---

## 목차

1. [CLI 인수 파싱 및 검증](#1-cli-인수-파싱-및-검증)
2. [세션 관리](#2-세션-관리)
3. [확장 시스템](#3-확장-시스템)
4. [TUI 통합](#4-tui-통합)
5. [스킬 시스템 및 프롬프트 템플릿](#5-스킬-시스템-및-프롬프트-템플릿)
6. [미디어 처리](#6-미디어-처리)
7. [인프라 레이어](#7-인프라-레이어)
8. [컨텍스트 관리](#8-컨텍스트-관리)
9. [패키지 매니저](#9-패키지-매니저)
10. [OAuth 흐름](#10-oauth-흐름)
11. [RPC 모드](#11-rpc-모드)
12. [설정/구성 계층화](#12-설정구성-계층화)
13. [종합 평가 및 우선순위](#13-종합-평가-및-우선순위)

---

## 1. CLI 인수 파싱 및 검증

**파일:** `cli.rs`, `main.rs`

### 1.1 `--thinking` 플래그 검증 불일치 — **Medium**

**위치:** `main.rs:125-133`

```rust
if let Some(ref level_str) = args.thinking {
    if let Some(level) = oxi_store::settings::parse_thinking_level(level_str) {
        settings.thinking_level = level;
    } else {
        anyhow::bail!(
            "Invalid thinking level: {}. Valid options: none, minimal, standard, thorough",
            level_str
        );
    }
}
```

**문제:** 에러 메시지에는 `none, minimal, standard, thorough`가 유효하다고 표시하지만, 실제 `ThinkingLevel` 열거형은 `Off, Minimal, Low, Medium, High, XHigh`입니다. 사용자가 혼란을 겪을 수 있습니다.

**개선:** 에러 메시지를 실제 유효한 값과 일치시키거나, `parse_thinking_level`이 지원하는 값을 동적으로 생성하세요.

### 1.2 `--extensions` 플래그 사용되지 않음 — **Low**

**위치:** `cli.rs:52-55`

```rust
#[arg(short = 'e', long = "extension", value_name = "PATH")]
pub extensions: Vec<PathBuf>,
```

`main.rs`에서 `args.extensions`를 읽는 코드가 없습니다. `--extension` 플래그로 전달된 경로가 무시됩니다.

**개선:** `main.rs`의 확장 로딩 로직에서 `args.extensions`를 `discover_extensions()`의 추가 경로로 전달하거나, 사용되지 않으면 플래그를 제거하세요.

### 1.3 `truncate()` 함수 UTF-8 안전하지 않음 — **Medium**

**위치:** `main.rs:460-464`

```rust
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}
```

`&s[..max_len.saturating_sub(3)]`는 바이트 단위 슬라이싱이며, 멀티바이트 문자(한국어, 이모지 등)의 중간에서 자를 경우 패닉이 발생합니다.

**개선:** `char_indices()`를 사용하여 문자 경계에서 자르도록 수정하세요.

### 1.4 `--no-session` 플래그 미구현 — **Low**

**위치:** `cli.rs:79-80`

```rust
#[arg(long)]
pub no_session: bool,
```

`main.rs`에서 이 플래그를 참조하는 곳이 없습니다.

**개선:** 구현하거나 제거하세요.

---

## 2. 세션 관리

**파일:** `app/agent_session.rs`, `app/agent_session_runtime.rs`

### 2.1 `InteractiveLoop`에서 `std::sync::mpsc` + `LocalSet` 교착 위험 — **High**

**위치:** `lib.rs:423-447`

```rust
let (tx, rx) = std::sync::mpsc::channel::<AgentEvent>();
let agent = Arc::clone(&self.app.agent);
let local = tokio::task::LocalSet::new();
local.spawn_local(async move {
    let _ = agent.run_with_channel(prompt, tx).await;
});
while let Ok(event) = rx.recv() { ... }
local.await;
```

`rx.recv()`가 동기 블로킹으로 현재 스레드를 점유하면, `LocalSet` 내의 future가 실행될 기회가 없어 교착 상태가 발생할 수 있습니다.

**개선:** `prompt_streaming()` 메서드처럼 `spawn_blocking` + `LocalSet` 패턴을 사용하거나, tokio 채널 기반으로 전환하세요.

### 2.2 `SessionListenerGuard` drop이 listener를 제거하지 않고 no-op으로 교체 — **Medium**

**위치:** `agent_session.rs:1388-1393`

```rust
impl Drop for SessionListenerGuard {
    fn drop(&mut self) {
        let mut listeners = self.listeners.write();
        if self.key < listeners.len() {
            listeners[self.key] = Box::new(|_| {});
        }
    }
}
```

리스너를 벡터에서 제거하는 대신 no-op 클로저로 교체합니다. 이는 메모리 누수는 아니지만, 등록/해제를 반복하면 벡터가 계속 커집니다.

**개선:** `Option<Box<...>>` 벡터로 변경하고 `None`으로 설정하거나, swap-remove + 인덱스 재매핑을 사용하세요.

### 2.3 `is_compacting()`이 `try_lock` 기반으로 구현되어 부정확할 수 있음 — **Medium**

**위치:** `agent_session.rs:325-332`

```rust
pub fn is_compacting(&self) -> bool {
    match self.compaction_abort.try_lock() {
        Ok(guard) => guard.is_some(),
        Err(_) => true,
    }
}
```

다른 이유로 `compaction_abort` 뮤텍스가 잠겨 있어도 compacting으로 간주합니다.

**개선:** 전용 `AtomicBool` 플래그를 추가하여 compaction 상태를 명시적으로 추적하세요.

### 2.4 `agent_session_runtime.rs`의 `fork()`가 실제 분기 데이터를 복사하지 않음 — **High**

**위치:** `agent_session_runtime.rs:284-297`

```rust
pub fn fork(&mut self, entry_id: &str, _position: ForkPosition) -> Result<()> {
    let session_dir = get_default_session_dir();
    let cwd_str = self.services.cwd.to_string_lossy().to_string();
    let mut session_manager = {
        let sm = SessionManager::create(&cwd_str, Some(&session_dir));
        sm
    };
    if let Err(e) = session_manager.branch(entry_id) {
        tracing::warn!("Branch to entry {} failed: {}", entry_id, e);
    }
    ...
}
```

새 `SessionManager`를 생성하지만, 현재 세션의 메시지/엔트리를 복사하지 않습니다. 결과적으로 fork된 세션은 빈 세션입니다.

**개선:** `SessionManager::fork_from()`을 사용하거나, 현재 세션의 엔트리를 지정된 `entry_id`까지 복사하는 로직을 추가하세요.

---

## 3. 확장 시스템

**파일:** `extensions/loading.rs`, `extensions/wasm.rs`, `extensions/registry.rs`

### 3.1 동적 라이브러리 의도적 메모리 누수 — **High**

**위치:** `extensions/loading.rs:87-90`

```rust
// IMPORTANT: We must keep the Library alive for the entire lifetime
// of the extension. Leak it intentionally
std::mem::forget(library);
```

`Library`를 `forget`하여 의도적으로 누수시킵니다. 확장을 동적 로드/언로드하는 경우 누적 메모리 누수가 발생합니다.

**개선:** `Arc<Library>`를 확장 객체와 함께 보관하거나, `Arc<(Library, Box<dyn Extension>)>` 패턴을 사용하세요. 확장 언로드 시 Library도 함께 해제되도록 보장해야 합니다.

### 3.2 WASM `oxi_exec` 호스트 함수 타임아웃 미구현 — **High**

**위치:** `extensions/wasm.rs:445-448`

```rust
// Note: true timeout enforcement requires async or kill-on-timeout logic.
// The timeout field is informational for now — commands run until completion.
let timed_out = false;
```

확장이 `timeout` 필드를 전달하지만 실제로는 무시됩니다. 악의적/버그 있는 확장이 명령을 무한정 실행할 수 있습니다.

**개선:** `std::process::Command`에 `.stdout(Stdio::piped())`를 설정하고, 별도 스레드에서 `wait_with_output()`을 실행한 후 타임아웃을 적용하세요.

### 3.3 WASM KV 스토어가 확장별 네임스페이싱 없이 글로벌 — **Medium**

**위치:** `extensions/wasm.rs:524-535`

```rust
static KV_STORE: LazyLock<parking_lot::RwLock<HashMap<String, String>>> = ...;
fn kv_store_get(key: &str) -> Option<String> { ... }
fn kv_store_set(key: &str, value: &str) { ... }
```

모든 확장이 동일한 글로벌 KV 스토어를 공유합니다. `kv_namespaced_get/set` 함수가 정의되어 있지만 `#[allow(dead_code)]`로 표시되어 사용되지 않습니다.

**개선:** 호스트 함수의 `UserData`에 확장 ID를 전달하고, 네임스페이스된 접근을 기본으로 사용하세요.

### 3.4 WASM 메모리 제한이 64페이지(4MB)로 설정됨 — **Low**

**위치:** `extensions/wasm.rs:691`

```rust
let manifest = extism::Manifest::new([wasm]).with_memory_max(64);
```

4MB는 일부 확장(특히 대량의 JSON 처리)에게 부족할 수 있습니다.

**개선:** 확장 manifest에서 메모리 제한을 설정할 수 있도록 하거나, 기본값을 128페이지(8MB)로 늘리세요.

### 3.5 `WasmExtensionManager`의 수동 `unsafe impl Send/Sync` — **Medium**

**위치:** `extensions/wasm.rs:656-657`

```rust
unsafe impl Send for WasmExtensionManager {}
unsafe impl Sync for WasmExtensionManager {}
```

`extism::Plugin`이 실제로 Send+Sync인지 확인 없이 수동 구현합니다. Plugin이 내부적으로 Send가 아닌 상태를 가지면 데이터 경쟁이 발생할 수 있습니다.

**개선:** extism의 `Plugin` 타입이 `Send+Sync`인지 확인하고, 그렇지 않다면 `Mutex`로 감싸서 자동으로 `Send+Sync`가 되도록 하세요.

### 3.6 확장 `Command` 타입에 실행 콜백이 없음 — **Medium**

**위치:** `extensions/types.rs:372-379`

```rust
pub struct Command {
    pub name: String,
    pub description: String,
    pub usage: String,
}
```

명령이 이름과 설명만 정의하고 실행 로직을 포함하지 않습니다. WASM 확장은 `register_commands()`를 통해 명령을 등록하지만, 실행은 `WasmExtensionManager::execute_command()`에서 별도로 처리합니다. 인프로세스 Rust 확장은 명령을 등록할 방법이 없습니다.

**개선:** `Command`에 `execute: Option<Arc<dyn Fn(&str) -> Result<String> + Send + Sync>>` 필드를 추가하세요.

---

## 4. TUI 통합

**파일:** `tui/app.rs`, `tui/handlers.rs`

### 4.1 에이전트 워커 스레드에서 `tokio::runtime::Builder::new_multi_thread()` 매 요청 생성 — **Critical**

**위치:** `tui/app.rs:266-268`

```rust
let agent_handle = std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Failed to build agent runtime");
```

세션 전환 시마다 새로운 Tokio 런타임이 생성됩니다. 런타임 생성은 비용이 많이 들며, 파일 디스크립터/스레드 누수 위험이 있습니다.

**개선:** 런타임을 한 번 생성하고 재사용하세요. 또는 기존 메인 런타임에서 에이전트 작업을 실행하세요.

### 4.2 포워더 스레드 분리 후 조인 불가 — **Medium**

**위치:** `tui/app.rs:371-375`

```rust
// The forwarder thread is detached and will clean up
// when it sees the channel disconnect.
let _ = forwarder_handle; // move ownership, don't block
```

포워더 스레드가 분리되어 세션 전환 시 이전 포워더가 여전히 실행 중일 수 있습니다. 이벤트가 새 세션의 `ui_rx`로 전달될 위험은 없지만 (채널이 다름), 리소스 정리가 보장되지 않습니다.

**개선:** 포워더 스레드에 타임아웃 기반 조인을 추가하거나, 종료 신호 채널을 전달하세요.

### 4.3 `snapshot_text_rendered` UTF-8 바이트 경계 처리 — **Medium**

**위치:** `tui/app.rs:564-572`

```rust
let byte_off = text.char_indices()
    .map(|(i, _)| i)
    .find(|&i| i >= self.snapshot_text_rendered)
    .unwrap_or(text.len());
```

`snapshot_text_rendered`는 **바이트** 단위로 저장되는데, `char_indices`의 비교 기준도 바이트입니다. 하지만 `snapshot_text_rendered`가 이전 텍스트의 `len()`으로 설정되므로, 텍스트가 교체되는 경우(예: 스트리밍 중 텍스트 수정) 오프셋이 무효화될 수 있습니다.

**개선:** 스냅샷 기반 렌더링에서 오프셋 대신 전체 텍스트 diff를 사용하거나, 스냅샷 교체 시 `snapshot_text_rendered`를 0으로 재설정하는 로직을 명시하세요.

### 4.4 `input_history` 최대 100개, 순환 버퍼 아님 — **Low**

**위치:** `tui/handlers.rs:78`

```rust
if state.input_history.len() > 100 { state.input_history.pop(); }
```

`.pop()`은 마지막 요소를 제거합니다. `Vec::insert(0, ...)`로 앞에 추가하고 있으므로, 실제로는 가장 오래된 항목이 아니라 마지막 항목이 제거됩니다.

**개선:** `VecDeque`를 사용하거나, `.pop()` 대신 `.remove(0)`을 사용하세요. (현재 논리적으로는 의도한 대로 동작하지 않습니다 — 101번째 삽입 시 마지막 항목이 사라집니다.)

---

## 5. 스킬 시스템 및 프롬프트 템플릿

**파일:** `skills/mod.rs`, `prompt/templates.rs`, `prompt/system_prompt.rs`

### 5.1 스킬 검색 시 동일 스킬이 중복 포함될 가능성 — **Low**

**위치:** `skills/mod.rs:143-176`

`search()`에서 `name_matches`, `desc_matches`로 분리한 후, 콘텐츠 검색 시 이미 `name_matches`나 `desc_matches`에 있는지 확인하지만, `desc_matches`에 동일 스킬이 두 번 추가될 수 있습니다 (설명과 콘텐츠 모두 매치되는 경우).

**개선:** `HashSet`을 사용하여 중복을 방지하세요.

### 5.2 시스템 프롬프트 빌드 로직 중복 — **Medium**

**위치:** `lib.rs:97-123` 및 `agent_session_runtime.rs:357-404`

`lib.rs`의 `build_system_prompt()`와 `agent_session_runtime.rs`의 `build_system_prompt()`가 유사하지만 다른 로직을 가집니다. `lib.rs` 버전은 `skill_contents`를 받아 처리하고, 런타임 버전은 `tool_snippets`와 `selected_tools`를 하드코딩합니다.

**개선:** 단일 `build_system_prompt` 함수로 통합하고, 매개변수로 커스터마이징 가능하게 하세요.

---

## 6. 미디어 처리

**파일:** `media/clipboard_image.rs`, `media/image_convert.rs`, `media/mime_detect.rs`, `media/file_processor.rs`

### 6.1 `mime_detect.rs` 파일 읽기 실패 시 기본값 `application/octet-stream` — **Low**

이 모듈은 파일 내용 기반 MIME 감지를 수행하며, 감지 실패 시 `application/octet-stream`을 반환합니다. 이는 안전한 기본값이지만, 텍스트 파일이 바이너리로 처리될 수 있습니다.

**개선:** 확장자 기반 fallback을 추가하고, 텍스트 파일에 대해 `text/plain`을 시도하세요.

### 6.2 `image_convert.rs`의 이미지 변환 — **Low**

이미지 변환 시 메모리 내에서 전체 이미지를 디코딩합니다. 대용량 이미지(예: 50MP 사진)의 경우 메모리 사용량이 매우 높을 수 있습니다.

**개선:** 최대 픽셀 수 제한을 설정하거나, 스트리밍 기반 변환을 고려하세요.

---

## 7. 인프라 레이어

**파일:** `infra/event_bus.rs`, `infra/shutdown.rs`, `infra/fs_watch.rs`, `infra/diagnostics.rs`

### 7.1 `ShutdownCoordinator`가 SIGINT 리스너를 한 번만 `listen()` 해야 함 — **Medium**

**위치:** `infra/shutdown.rs:34-53`

`listen()`이 호출될 때마다 새 `tokio::spawn`으로 리스너가 생성됩니다. 여러 번 호출하면 여러 SIGINT 핸들러가 등록됩니다.

**개선:** `listen()` 호출을 `new()`에 통합하거나, 중복 호출을 방지하는 가드를 추가하세요.

### 7.2 `event_bus.rs`의 구독자 락 경합 — **Medium**

`EventBus`가 `tokio::RwLock<HashMap<...>>`으로 구현되어 있습니다. 이벤트 발행 시 모든 구독자를 순회하면서 쓰기 락을 유지합니다. 구독자 콜백이 느리면 전체 이벤트 파이프라인이 지연됩니다.

**개선:** `DashMap`을 사용하거나, 이벤트 발행을 락 없이 clone 후 dispatch 하세요.

---

## 8. 컨텍스트 관리

**파일:** `context/auto_compaction.rs`, `context/branch_summarization.rs`, `context/compaction_utils.rs`

### 8.1 `auto_compaction.rs`의 토큰 추정이 부정확 — **Medium**

**위치:** `auto_compaction.rs:317-319`

```rust
fn estimate_tokens(&self, messages: &[AgentMessage]) -> usize {
    messages.iter().map(|msg| {
        (msg.content.len() / 4).max(1)
    }).sum()
}
```

`msg.content.len()`은 **바이트** 수이며, 한국어/중국어/이모지 문자는 3-4바이트를 차지합니다. 결과적으로 한국어 텍스트의 토큰 수를 과소 평가합니다.

**개선:** `char` 수를 기반으로 추정하거나, tiktoken 기반 추정기를 사용하세요.

### 8.2 `build_summarization_prompt`에서 메시지 잘림이 바이트 기반 — **Medium**

**위치:** `auto_compaction.rs:341-344`

```rust
let content = if msg.content.len() > 500 {
    format!("{}...", &msg.content[..500])
} else {
    msg.content.clone()
};
```

`&msg.content[..500]`은 멀티바이트 문자 중간에서 자를 수 있어 패닉을 유발합니다.

**개선:** `char_indices()`를 사용하세요.

### 8.3 `agent_session.rs`와 `auto_compaction.rs`의 CompactionReason 불일치 — **Medium**

**위치:** `agent_session.rs:53-60` vs `auto_compaction.rs:130-142`

`agent_session.rs`는 `Manual/Threshold/Overflow`를 정의하고, `auto_compaction.rs`는 `Manual/Automatic/Overflow/Iteration`을 정의합니다. 두 타입이 서로 다르며 변환 로직이 없습니다.

**개선:** `CompactionReason`을 단일 위치에 정의하고 공유하세요.

---

## 9. 패키지 매니저

**파일:** `storage/packages.rs` (2917줄)

### 9.1 npm 패키지 설치 시 npm CLI 호출의 명령 주입 위험 — **Critical**

**위치:** `storage/packages.rs` (패키지 설치 로직)

npm 패키지 이름이 shell 명령에 직접 전달될 가능성이 있습니다. 사용자 입력인 패키지 이름을 검증 없이 사용하면 명령 주입이 가능합니다.

**개선:** 패키지 이름을 엄격하게 검증(알파벳, 숫자, 하이픈, 슬래시, `@`만 허용)하고, `std::process::Command`에 인자 배열로 전달하세요 (절대 셸 문자열 보간 사용 금지).

### 9.2 패키지 업데이트 시 부분 실패 처리 — **Medium**

**위치:** `main.rs:177-184`

```rust
for pkg_name in &packages {
    match mgr.update(pkg_name) {
        Ok(manifest) => { println!("Updated {} to v{}", manifest.name, manifest.version); }
        Err(e) => { eprintln!("Failed to update {}: {}", pkg_name, e); }
    }
}
```

일부 패키지만 업데이트되고 일부는 실패해도 전체 명령은 성공으로 종료됩니다.

**개선:** 실패한 패키지가 있으면 exit code 1로 종료하거나, 트랜잭션 기반 업데이트를 고려하세요.

---

## 10. OAuth 흐름

**파일:** `oauth_server.rs`

### 10.1 OAuth 콜백 URL 파싱 시 URL 디코딩 미적용 — **Medium**

**위치:** `oauth_server.rs:232-238`

```rust
for pair in query.split('&') {
    let mut parts = pair.split('=');
    let key = parts.next()?;
    let value = parts.next()?.replace("%3D", "=").replace("%26", "&");
```

`%3D`와 `%26`만 수동으로 디코딩합니다. `%20` (공백), `%2F` (슬래시) 등 다른 퍼센트 인코딩은 처리하지 않습니다.

**개선:** `url::form_urlencoded::parse()` 또는 `percent_encoding::percent_decode_str()`을 사용하세요.

### 10.2 `authorize_with_browser`에서 인증 URL에 redirect_uri 포함 안 함 — **High**

**위치:** `oauth_server.rs:260-274`

```rust
pub async fn authorize_with_browser(auth_url: &str) -> Result<OAuthCallbackData> {
    open_browser(auth_url)?;
    let server = OAuthCallbackServer::with_available_port()?;
    let port = server.port();
    server.start().await
}
```

`auth_url`이 이미 `redirect_uri`를 포함하고 있다고 가정하지만, 콜백 서버의 포트는 이 함수 내에서 결정됩니다. 호출자가 미리 포트를 알 수 없으므로, `redirect_uri`가 `auth_url`과 일치하지 않을 가능성이 있습니다.

**개선:** 콜백 서버를 먼저 생성하고, redirect_uri를 auth_url에 주입한 후 브라우저를 여세요.

### 10.3 OAuth 서버 타임아웃 10분 — **Low**

**위치:** `oauth_server.rs:182`

600초 타임아웃은 합리적이지만, 사용자가 긴 인증 과정을 거치는 경우 부족할 수 있습니다. 설정 가능하게 하는 것이 좋습니다.

---

## 11. RPC 모드

**파일:** `rpc_mode/handlers.rs`, `rpc_mode/protocol.rs`, `rpc_mode/state.rs`

### 11.1 RPC 핸들러가 실제 에이전트 로직을 호출하지 않음 — **Critical**

**위치:** `rpc_mode/handlers.rs:76-91`

```rust
RpcCommand::Prompt { id, message: _, images, streaming_behavior: _ } => {
    let _image_sources = RpcServer::parse_images(images);
    server.update_session_state(|s| { s.is_streaming = true; s.pending_message_count += 1; });
    server.emit_event(RpcEvent::AgentStart);
    RpcResponse::Response { id, command: "prompt".to_string(), success: true, ... }
}
```

`Prompt` 명령이 실제로 에이전트를 실행하지 않습니다. 상태만 업데이트하고 즉시 성공 응답을 반환합니다. `Bash`, `Compact`, `GetMessages` 등 대부분의 핸들러가 스텁입니다.

**개선:** `_app` 매개변수를 사용하여 실제 에이전트 로직을 호출하거나, 핸들러가 비동기 작업을 시작하도록 구현하세요.

### 11.2 RPC `Bash` 명령이 명령 주입에 취약 — **Critical**

**위치:** `rpc_mode/handlers.rs:290-302`

```rust
RpcCommand::Bash { id, command } => {
    let output_result = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .output();
```

RPC 클라이언트가 임의의 셸 명령을 실행할 수 있습니다. WASM 확장의 `oxi_exec`와 달리 차단 목록이나 권한 검사가 없습니다.

**개선:** 최소한 WASM 확장과 동일한 차단 목록을 적용하고, 허용된 명령 화이트리스트 또는 샌드박스를 사용하세요.

### 11.3 RPC 서버가 동기식 stdin 블로킹으로 비동기 이벤트를 놓침 — **High**

**위치:** `rpc_mode/handlers.rs:34-51`

```rust
loop {
    let mut read_buf = String::new();
    match input.read_line(&mut read_buf) {
        ...
    }
}
```

`input.read_line()`이 블로킹되어 있어, 에이전트 이벤트가 동시에 처리되지 않습니다. 이벤트 포워딩 태스크가 있지만, stdin 루프가 긴 명령을 처리하는 동안 이벤트 큐가 쌓일 수 있습니다.

**개선:** `tokio::io::BufReader<tokio::io::Stdin>`을 사용하여 비동기 stdin 읽기를 구현하고, `tokio::select!`로 이벤트와 입력을 동시에 처리하세요.

### 11.4 JSON-RPC 2.0의 `id`가 `Value::Null`일 때 응답 전송 — **Low**

**위치:** `rpc_mode/protocol.rs:393`

JSON-RPC 2.0 스펙에 따르면 `id`가 없는 요청은 notification이며 응답을 보내지 않아야 합니다. 하지만 현재 구현은 항상 응답을 보냅니다.

**개선:** `id`가 `None`/`Null`이면 응답을 건너뛰세요.

---

## 12. 설정/구성 계층화

**파일:** `main.rs`, `agent_session_runtime.rs`

### 12.1 `AuthStorage::new()`가 여러 번 생성됨 — **Medium**

**위치:** `main.rs:55` (초기 로딩), `main.rs:370` (models 명령어), `agent_session_runtime.rs:131` (서비스 생성)

동일 프로세스 내에서 `AuthStorage::new()`가 여러 번 호출됩니다. 각 인스턴스가 같은 파일을 읽지만 독립적인 상태를 가집니다. 한 인스턴스에서의 변경이 다른 인스턴스에 반영되지 않을 수 있습니다.

**개선:** `AuthStorage`를 싱글톤으로 관리하거나, `Arc<AuthStorage>`로 공유하세요.

### 12.2 `Settings`가 CLI 오버라이드 후 `merge_cli()`로 임시 수정만 됨 — **Low**

**위치:** `main.rs:44`

```rust
settings.merge_cli(args.model.clone(), args.provider.clone());
```

CLI 오버라이드가 세션에만 적용되고 저장되지 않습니다. 이는 올바른 동작이지만, 사용자가 `--model` 플래그를 사용할 때 세션 종료 후 설정이 유지되지 않는다는 점을 명시적으로 문서화해야 합니다.

### 12.3 `handle_config_reset`이 `settings_path()` 실패를 무시 — **Low**

**위치:** `main.rs:398-405`

```rust
if let Ok(settings_path) = oxi_store::settings::Settings::settings_path() {
    if settings_path.exists() {
        std::fs::remove_file(&settings_path)?;
    }
}
```

`settings_path()`가 실패하면 조용히 무시됩니다. 설정 파일이 존재하지만 찾을 수 없는 경우 리셋되지 않습니다.

---

## 13. 종합 평가 및 우선순위

### Critical (즉시 수정 필요)

| # | 문제 | 파일:줄 |
|---|------|---------|
| 11.1 | RPC 핸들러가 에이전트를 호출하지 않음 (스텁) | `rpc_mode/handlers.rs:76` |
| 9.1 | npm 패키지 설치 시 명령 주입 위험 | `storage/packages.rs` |
| 11.2 | RPC `Bash` 명령에 보안 검사 없음 | `rpc_mode/handlers.rs:290` |
| 4.1 | 세션 전환 시마다 Tokio 런타임 재생성 | `tui/app.rs:266` |

### High (가까운 시일 내 수정)

| # | 문제 | 파일:줄 |
|---|------|---------|
| 3.1 | 동적 라이브러리 `forget()` 메모리 누수 | `extensions/loading.rs:87` |
| 3.2 | WASM `oxi_exec` 타임아웃 미구현 | `extensions/wasm.rs:445` |
| 2.1 | `InteractiveLoop` 동기 채널 교착 위험 | `lib.rs:423` |
| 2.4 | `fork()`가 실제 데이터를 복사하지 않음 | `agent_session_runtime.rs:284` |
| 10.2 | OAuth redirect_uri 포트 불일치 | `oauth_server.rs:260` |
| 11.3 | RPC 서버 동기 stdin 블로킹 | `rpc_mode/handlers.rs:34` |

### Medium (계획된 반영)

| # | 문제 | 파일:줄 |
|---|------|---------|
| 1.1 | `--thinking` 에러 메시지 불일치 | `main.rs:130` |
| 1.3 | `truncate()` UTF-8 패닉 | `main.rs:462` |
| 2.2 | 리스너 가드가 벡터에서 제거 안 함 | `agent_session.rs:1388` |
| 2.3 | `is_compacting()` 부정확 | `agent_session.rs:325` |
| 3.3 | KV 스토어 글로벌 공유 | `extensions/wasm.rs:524` |
| 3.5 | 수동 `unsafe impl Send/Sync` | `extensions/wasm.rs:656` |
| 3.6 | `Command`에 실행 콜백 없음 | `extensions/types.rs:372` |
| 4.3 | 스냅샷 텍스트 오프셋 무효화 위험 | `tui/app.rs:564` |
| 5.2 | 시스템 프롬프트 빌드 중복 | `lib.rs:97`, `agent_session_runtime.rs:357` |
| 7.1 | SIGINT 리스너 다중 등록 | `infra/shutdown.rs:34` |
| 8.1 | 토큰 추정 바이트 기반 부정확 | `auto_compaction.rs:317` |
| 8.2 | 메시지 잘림 UTF-8 패닉 | `auto_compaction.rs:341` |
| 8.3 | CompactionReason 타입 중복 | `agent_session.rs:53`, `auto_compaction.rs:130` |
| 10.1 | OAuth URL 디코딩 불완전 | `oauth_server.rs:235` |
| 12.1 | AuthStorage 다중 인스턴스 | `main.rs:55` |

### Low (향후 개선)

| # | 문제 | 파일:줄 |
|---|------|---------|
| 1.2 | `--extensions` 플래그 미사용 | `cli.rs:52` |
| 1.4 | `--no-session` 미구현 | `cli.rs:79` |
| 3.4 | WASM 메모리 제한 4MB | `extensions/wasm.rs:691` |
| 4.2 | 포워더 스레드 분리 | `tui/app.rs:371` |
| 4.4 | input_history Vec::pop 논리 오류 | `tui/handlers.rs:78` |
| 5.1 | 스킬 검색 중복 가능성 | `skills/mod.rs:143` |

---

## 아키텍처 총평

### 강점

1. **확장 시스템 설계**: WASM + 인프로세스 이중 확장 아키텍처는 유연하고, SSRF 방어, 경로 검증, 명령 차단 목록 등 보안 계층이 잘 설계되어 있습니다.
2. **TUI 이벤트 파이프라인**: pi-mono 패턴의 메시지 스냅샷 기반 렌더링은 텍스트/도구 호출/사고 블록 분리를 올바르게 처리합니다.
3. **세션 관리**: 트리 기반 세션, 포크, 브랜치, 자동 compaction은 잘 구조화되어 있습니다.
4. **RPC 프로토콜**: JSON-RPC 2.0과 네이티브 JSONL 듀얼 지원, 엄격한 JSONL 프레이밍(`JsonlLineReader`)은 견고합니다.
5. **테스트 커버리지**: 대부분의 모듈에 단위 테스트가 포함되어 있습니다.

### 개선 영역

1. **RPC 모드 미완성**: 핸들러가 대부분 스텁이며, 실제 에이전트와 연결되지 않았습니다. 프로덕션 사용 전 완전한 재작성이 필요합니다.
2. **코드 중복**: 시스템 프롬프트 빌더, `CompactionReason` 타입, `AuthStorage` 인스턴스 생성 등이 여러 위치에 중복됩니다.
3. **UTF-8 안전성**: 여러 위치에서 `&str[..n]` 슬라이싱을 사용하여 멀티바이트 문자 환경에서 패닉 위험이 있습니다.
4. **비동기 패턴 불일치**: 일부 모듈은 동기 채널(`std::sync::mpsc`)을 사용하고 다른 모듈은 비동기 채널(`tokio::sync::mpsc`)을 사용하여 교착 위험이 있습니다.
5. **런타임 수명 관리**: TUI의 세션 전환 루프에서 Tokio 런타임을 반복 생성하는 것은 리소스 낭비입니다.

---

*이 보고서는 정적 코드 분석을 기반으로 작성되었으며, 런타임 동작이나 외부 의존성의 정확성은 보장하지 않습니다.*

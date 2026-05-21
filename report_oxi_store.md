# oxi-store 종합 정적 분석 보고서

**분석 대상:** `/Volumes/MERCURY/PROJECTS/oxi/oxi-store/src/`  
**분석일:** 2026-05-14  
**크레이트 버전:** 0.11.0  
**분석 파일:** 10개 `.rs` 소스 파일

---

## 목차

1. [요약](#1-요약)
2. [세션 영속성 (Session Persistence)](#2-세션-영속성-session-persistence)
3. [설정 시스템 (Settings System)](#3-설정-시스템-settings-system)
4. [인증 저장소 (Auth Storage)](#4-인증-저장소-auth-storage)
5. [모델 레지스트리 (Model Registry)](#5-모델-레지스트리-model-registry)
6. [모델 리졸버 (Model Resolver)](#6-모델-리졸버-model-resolver)
7. [파일 I/O 패턴 (File I/O)](#7-파일-io-패턴-file-io)
8. [데이터 마이그레이션 (Data Migration)](#8-데이터-마이그레이션-data-migration)
9. [세션 내비게이션 (Session Navigation)](#9-세션-내비게이션-session-navigation)
10. [동시성 안전성 (Concurrency Safety)](#10-동시성-안전성-concurrency-safety)
11. [CWD 추적 (CWD Tracking)](#11-cwd-추적-cwd-tracking)
12. [종합 평가](#12-종합-평가)

---

## 1. 요약

oxi-store는 AI 코딩 어시스턴트 oxi의 핵심 상태 관리 크레이트로, 세션 관리(JSONL), 설정(레이어드 TOML/JSON), 인증 저장소, 모델 레지스트리를 제공합니다. 전반적으로 견고한 구조를 갖추고 있으나, **파일 쓰기 원자성, 동시성 제어, 데이터 무결성** 측면에서 개선이 필요한 영역이 확인되었습니다.

### 심각도별 이슈 요약

| 심각도 | 건수 | 설명 |
|--------|------|------|
| **Critical** | 3 | 데이터 손실 위험, 인증 정보 평문 노출, 데드락 가능성 |
| **High** | 7 | 원자적 쓰기 누락, 레이스 컨디션, 비효율적 메모리 사용 |
| **Medium** | 9 | 불완전한 검증, 누락된 에러 처리, API 일관성 |
| **Low** | 6 | 코드 품질, 문서화, 성능 최적화 |

---

## 2. 세션 영속성 (Session Persistence)

### 2.1 JSONL 포맷 및 손상 저항성

**파일:** `session.rs`

세션은 JSONL(JSON Lines) 포맷으로 저장되며, 첫 줄은 `SessionHeader`, 이후 줄은 `FileEntry` 열거형입니다.

**👍 장점:**
- 줄 단위 파싱으로 부분 손상 시에도 복구 가능 (`load_entries_from_file`, session.rs:2838-2858)
- `#[serde(untagged)]` 열거형으로 유연한 디코딩 (session.rs:324)
- 버전 관리(`CURRENT_SESSION_VERSION = 3`)를 통한 마이그레이션 지원 (session.rs:14)

**이슈 #1 — Critical: `_rewrite_file()`의 비원자적 쓰기**
- **위치:** session.rs:503-517
- **문제:** 전체 파일을 `fs::write(file, content)`로 직접 덮어씁니다. 쓰기 도중 크래시/전원 손실 시 파일이 반쯤 쓰인 상태로 남아 세션 데이터가 손실될 수 있습니다.
  ```rust
  fn _rewrite_file(&self) {
      // ...
      let content = /* ... */.join("\n") + "\n";
      if let Err(e) = fs::write(file, content) {  // ← 비원자적!
          tracing::warn!("Failed to rewrite session file {}: {}", file, e);
      }
  }
  ```
- **개선 제안:** 임시 파일에 쓴 후 `fs::rename()`으로 원자적 교체:
  ```rust
  let tmp = format!("{}.tmp", file);
  fs::write(&tmp, &content)?;
  fs::rename(&tmp, file)?;
  ```

**이슈 #2 — High: `_persist()`의 비원자적 append 플러시**
- **위치:** session.rs:462-497
- **문제:** `flushed == false`일 때 전체 엔트리를 순차적으로 `writeln!`으로 씁니다. 중간에 실패하면 파일이 불완전한 상태가 됩니다.
- **개선 제안:** 임시 파일에 전체를 쓴 뒤 rename으로 교체하거나, 최소한 `BufWriter`를 사용하여 버퍼링 후 한 번에 flush.

**이슈 #3 — Medium: `_persist()`의 `has_assistant` 체크 논리**
- **위치:** session.rs:467-472
- **문제:** assistant 메시지가 없으면 `_persist()`가 조기 반환합니다. 이는 "첫 assistant 응답 전에는 모든 메시지를 메모리에 보관"하겠다는 의도이지만, 긴 사용자 대화 후 크래시가 발생하면 모든 메시지가 손실됩니다.
- **개선 제안:** assistant 대기 여부와 무관하게 주기적으로(예: N개 메시지마다) 디스크에 flush하는 메커니즘 추가.

**이슈 #4 — Medium: `load_entries_from_file`의 조용한 무시**
- **위치:** session.rs:2838-2858
- **문제:** JSONL 파싱 실패 시 `continue`로 조용히 건너뜁니다. 손상된 줄의 존재를 사용자에게 알릴 방법이 없습니다.
  ```rust
  match serde_json::from_str::<FileEntry>(&line) {
      Ok(entry) => entries.push(entry),
      Err(_) => continue,  // ← 조용히 무시
  }
  ```
- **개선 제안:** 실패한 줄 수를 카운트하여 로그에 경고를 남기고, 필요시 사용자에게 알림.

### 2.2 세션 헤더 검증

**이슈 #5 — Low: 파일 경로 기반 세션 ID 충돌 가능성**
- **위치:** session.rs:469-471
- **문제:** 파일명에 `session_id[..8]` (UUID 앞 8자리)를 사용합니다. 16진수 8자리는 약 43억 개의 조합이 가능하나, 세션 수가 많아지면 파일명 충돌 위험이 있습니다.
- **개선 제안:** 파일명에 전체 UUID를 사용하거나, 충돌 감지 시 재시도 로직 추가.

---

## 3. 설정 시스템 (Settings System)

### 3.1 레이어드 설정

**파일:** `settings.rs`, `settings_validation.rs`

설정은 5개 레이어로 구성됩니다:
1. 빌트인 기본값 → 2. 글로벌 `~/.oxi/settings.toml` → 3. 프로젝트 `.oxi/settings.toml` → 4. 환경변수(현재 비활성화) → 5. CLI 인자

**👍 장점:**
- JSON/TOML 듀얼 포맷 지원 (settings.rs:146-187)
- `merge_json_values`를 통한 깊은 병합 (settings.rs:1645-1663)
- JSON이 TOML보다 우선순위 (settings.rs:160)
- `save()` 메서드의 원자적 쓰기 (tmp → rename) (settings.rs:652-660)

**이슈 #6 — High: `save()`의 임시 파일 확장자 충돌**
- **위치:** settings.rs:658
- **문제:** `path.with_extension("tmp")`를 사용합니다. `.toml` → `.tmp`, `.json` → `.tmp`로 변경됩니다. 동시에 두 프로세스가 같은 설정 파일을 쓰면 `.tmp` 파일이 충돌합니다.
  ```rust
  let tmp_path = path.with_extension("tmp");
  fs::write(&tmp_path, &content)?;
  fs::rename(&tmp_path, &path)?;
  ```
- **개선 제안:** 임시 파일명에 PID나 난수를 포함:
  ```rust
  let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
  ```

**이슈 #7 — High: `layer_file()`에서 `serde_json::to_value` → `from_value` 왕복 변환**
- **위치:** settings.rs:574-582
- **문제:** 기본 설정을 JSON Value로 직렬화하고, 오버레이를 병합한 뒤 다시 역직렬화합니다. 이 과정에서 TOML→JSON 변환 시 정밀도 손실(TOML float → JSON float)이나 타입 변환 문제가 발생할 수 있습니다.
- **개선 제안:** TOML 오버레이를 `Settings` 구조체로 직접 파싱하여 필드별 덮어쓰기를 수행하거나, 최소한 변환 실패 시 명확한 에러 메시지 제공.

**이슈 #8 — Medium: 환경변수 오버라이드 완전 비활성화**
- **위치:** settings.rs:600-604
- **문제:** `apply_env()`가 완전 no-op입니다. CI/CD 환경에서 `OXI_MODEL`, `OXI_PROVIDER` 등을 설정할 수 없습니다. 문서에는 "CI/CD 호환성"을 위해 유지한다고 되어 있지만 실제로는 작동하지 않습니다.
  ```rust
  pub fn apply_env(&mut self) {
      // No-op: environment variable overrides are disabled.
  }
  ```
- **개선 제안:** CI 감지(`CI=true` 환경변수) 시에만 환경변수 오버라이드를 활성화하거나, 기능을 완전히 제거하고 문서에서도 삭제.

### 3.2 설정 검증

**파일:** `settings_validation.rs`

**👍 장점:**
- 온도 범위 검증 (0.0~2.0) (settings_validation.rs:46-60)
- max_tokens 최소값/경고 (settings_validation.rs:62-87)
- 레거시 필드(`temperature: f32`, `max_tokens: u32`)도 검증

**이슈 #9 — Medium: `default_provider` / `default_model` 조합 검증 누락**
- **위치:** settings_validation.rs
- **문제:** `default_provider`만 설정되고 `default_model`이 없는 경우(또는 그 반대)를 검증하지 않습니다. 이는 런타임에 모델을 찾을 수 없는 에러로 이어집니다.
- **개선 제안:** 둘 중 하나만 설정된 경우 경고(warning)를 추가:
  ```rust
  if self.default_provider.is_some() != self.default_model.is_some() {
      report.warnings.push(ValidationWarning {
          field: "default_provider/default_model".into(),
          message: "Both should be set together for proper model resolution".into(),
      });
  }
  ```

**이슈 #10 — Medium: 중복 필드 존재 (`temperature`/`default_temperature`, `max_tokens`/`max_response_tokens`)**
- **위치:** settings.rs:254-265
- **문제:** `temperature: Option<f32>`와 `default_temperature: Option<f64>`, `max_tokens: Option<u32>`와 `max_response_tokens: Option<usize>`가 공존합니다. `effective_temperature()`와 `effective_max_tokens()`에서 하나를 우선하지만, 사용자에게 혼란을 줍니다.
- **개선 제안:** 레거시 필드를 `#[serde(alias)]`로 처리하고 단일 필드로 통합. 또는 `#[serde(skip_serializing)]`로 레거시 필드의 직렬화를 방지.

### 3.3 설정 마이그레이션

**파일:** settings.rs:699-753

**👍 장점:**
- v0→v4, v1→v4, v2→v4, v3→v4 마이그레이션 경로가 명확히 정의됨
- v3→v4에서 `default_model`의 `"provider/model"` 형식을 분리하는 스마트 마이그레이션
- 미래 버전(>`SETTINGS_VERSION`) 거부 로직

**이슈 #11 — Low: 마이그레이션 후 자동 저장 누락**
- **위치:** settings.rs `migrate()` 호출 이후 (settings.rs:556)
- **문제:** `load()`에서 마이그레이션이 수행되지만 자동으로 `save()`가 호출되지 않습니다. 다음 로드 시 동일한 마이그레이션이 반복됩니다.
- **개선 제안:** 마이그레이션이 발생한 경우 자동으로 디스크에 저장.

---

## 4. 인증 저장소 (Auth Storage)

**파일:** `auth_storage.rs`, `auth_guidance.rs`

### 4.1 아키텍처

`AuthStorage`는 다중 레이어 자격증명 조회를 제공합니다:
1. 런타임 오버라이드 (CLI `--api-key`)
2. `auth.json`의 저장된 API 키
3. OAuth 토큰 (자동 갱신 인식)
4. 세션 토큰
5. 폴백 리졸버

**이슈 #12 — Critical: `auth.json`에 API 키가 평문으로 저장됨**
- **위치:** auth_storage.rs:347-353 (`persist()`)
- **문제:** `serde_json::to_string_pretty(&*creds)`로 모든 자격증명이 평문 JSON으로 디스크에 저장됩니다. 파일 권한은 `0o600`으로 설정되지만 (auth_storage.rs:214-218), 이는 운영체제 수준의 보호만 제공하며:
  - 백업 도구가 권한을 유지하지 않을 수 있음
  - 디스크 검사 도구에 노출 가능
  - 공유 파일 시스템에서 위험
  ```rust
  fn persist(&self) {
      if let Some(ref storage) = self.file_storage {
          let creds = self.credentials.read();
          if let Ok(json) = serde_json::to_string_pretty(&*creds) {
              if let Err(e) = storage.write(&json) { /* ... */ }
          }
      }
  }
  ```
- **개선 제안:** 
  1. OS 키링(keyring)을 기본 백엔드로 사용 (현재 `#[cfg(feature = "keyring")]`으로 선택적)
  2. 파일 저장 시 AES-256-GCM 등으로 암호화 (기기 고유 키 또는 사용자 마스터 비밀번호 사용)
  3. 최소한 API 키의 마스킹된 형태를 로그에 출력

**이슈 #13 — High: `persist()`가 쓰기 오류를 무시**
- **위치:** auth_storage.rs:350-353
- **문제:** `storage.write(&json)` 실패 시 `record_error()`만 호출하고 호출자에게 에러를 전파하지 않습니다. `set_api_key()` 등의 메서드가 `Result`가 아닌 `()`를 반환합니다.
  ```rust
  pub fn set_api_key(&self, provider: &str, key: String) {
      self.credentials.write().insert(/* ... */);
      self.persist();  // ← 실패해도 알 수 없음
  }
  ```
- **개선 제안:** `persist()`가 `Result<()>`를 반환하게 하고, 최소한 로그에 경고를 남기며, `drain_errors()`를 호출자가 확인하도록 가이드.

**이슈 #14 — High: `FileAuthStorage`에 캐시 일관성 문제**
- **위치:** auth_storage.rs:169-171, 195-203
- **문제:** `read()`는 성공 시 `cache`를 갱신하지만, 외부에서 파일이 수정된 경우 `cache`와 실제 파일 내용이 불일치합니다. `reload()`는 있지만 자동 호출되지 않습니다.
- **개선 제안:** 파일 수정 시간(mtime)을 확인하여 캐시를 무효화하거나, 매번 파일에서 읽도록 캐시를 제거.

**이슈 #15 — Medium: OAuth 만료 판단의 경계 조건**
- **위치:** auth_storage.rs:80-84
- **문제:** `*expires_at <= now`로 만료를 판단합니다. `expires_at == now`일 때 이미 만료된 것으로 간주됩니다. 이는 `needs_refresh()`의 `expires_at <= now + 60`과 함께 경계 조건에서 토큰이 갱신되지 않을 수 있습니다.
  ```rust
  pub fn is_expired(&self) -> bool {
      match self {
          AuthCredential::OAuth { expires_at, .. } => {
              let now = now_secs();
              *expires_at <= now  // ← expires_at == now일 때 만료
          }
          // ...
      }
  }
  ```
- **개선 제안:** `*expires_at < now`로 변경하거나, `needs_refresh()`와 `is_expired()`의 로직을 통합.

**이슈 #16 — Medium: 키링 기능이 기본적으로 비활성화**
- **위치:** auth_storage.rs:384-405
- **문제:** `keyring` 기능이 `#[cfg(feature = "keyring")]`으로 보호되어 있으며, `Cargo.toml`에도 기본 feature로 포함되지 않았습니다.
- **개선 제안:** 기본 feature로 `keyring`을 포함하고, 파일 저장소를 폴백으로 사용.

---

## 5. 모델 레지스트리 (Model Registry)

**파일:** `model_registry.rs`

### 5.1 아키텍처

`ModelRegistry`는 다음을 관리합니다:
- 빌트인 모델 (`oxi_ai::model_db`)
- 커스텀 모델 (`models.json`)
- 동적 프로바이더 등록 (확장)
- API 키 리졸루션

**👍 장점:**
- `ProviderOverride` / `ModelOverride`를 통한 세밀한 오버라이드
- `models.json` 검증 (`validate_config()`)
- 동적 프로바이더 등록/해제 지원

**이슈 #17 — High: `models.json`의 `apiKey` 필드 보안 문제**
- **위치:** model_registry.rs:483-519 (`load_custom_models`)
- **문제:** `models.json`에 `"apiKey": "sk-..."` 형태로 API 키를 평문 저장할 수 있습니다. 이 파일은 보통 버전 관리에 포함되거나 공유될 수 있습니다.
- **개선 제안:** `apiKey` 필드를 환경변수 참조(`"$ENV_VAR_NAME"`)만 지원하도록 제한하거나, 경고를 표시.

**이슈 #18 — High: `resolve_config_value()`의 명령어 실행 (명령 주입)**
- **위치:** model_registry.rs:292-316
- **문제:** `!` 접두사가 있는 값은 셸 명령어로 실행됩니다. `models.json`의 `apiKey` 필드에 `!rm -rf /` 같은 악의적인 값을 넣을 수 있습니다.
  ```rust
  fn resolve_config_value(value: &str) -> Option<String> {
      if value.starts_with('!') {
          let cmd = &value[1..];
          let output = std::process::Command::new("sh")
              .arg("-c")
              .arg(cmd)  // ← 임의 명령어 실행!
              .output()
              .ok()?;
          // ...
      }
  }
  ```
- **개선 제안:** 명령어 실행 기능을 제거하거나, 허용된 명령어 패턴으로 화이트리스트를 도입. 최소한 `models.json` 로드 시 경고를 표시.

**이슈 #19 — Medium: `resolve_model()`의 모호한 매칭**
- **위치:** model_registry.rs:555-580
- **문제:** 슬래시가 없는 모델 ID 검색 시, 여러 프로바이더에 같은 ID가 있으면 "첫 번째"를 반환합니다. 어떤 것이 "첫 번째"인지는 보장되지 않습니다.
  ```rust
  // Multiple matches — prefer first
  if !matches.is_empty() {
      return Some(matches[0].clone());  // ← 비결정적
  }
  ```
- **개선 제안:** 다중 매치 시 모호성 경고를 반환하거나, 사용자에게 선택지를 제공.

**이슈 #20 — Low: `default_base_url_for_provider()`의 하드코딩된 URL**
- **위치:** model_registry.rs:860-876
- **문제:** 프로바이더 기본 URL이 하드코딩되어 있습니다. URL 변경 시 코드 수정이 필요합니다.
- **개선 제안:** `model_db`에 기본 URL을 저장하고 참조하도록 변경.

---

## 6. 모델 리졸버 (Model Resolver)

**파일:** `model_resolver.rs`

### 6.1 모델 해석 로직

모델 패턴 해석 순서:
1. 정확한 매치 (ID 또는 full_id)
2. `provider/model` 포맷
3. 부분 문자열 매치 (ambiguous 시 alias 우선)
4. 퍼지 매치 (대소문자 무시 포함)

**👍 장점:**
- Thinking level 매핑 (`get_thinking_level_map`)으로 모델별 최적 thinking 모델 선택
- `find_initial_model()`의 체계적 폴백 체인
- `restore_model_from_session()`의 인증 검증 포함

**이슈 #21 — Medium: `is_alias()`의 `Regex::new` 반복 생성**
- **위치:** model_resolver.rs:159-165
- **문제:** `is_alias()`가 호출될 때마다 `Regex::new(r"-\d{8}$")`를 생성합니다. `Regex::new`는 비용이 큰 작업입니다.
  ```rust
  fn is_alias(id: &str) -> bool {
      let date_pattern = regex::Regex::new(r"-\d{8}$").ok();
      // ...
  }
  ```
- **개선 제안:** `lazy_static!` 또는 `std::sync::LazyLock`으로 정규식을 한 번만 컴파일:
  ```rust
  static DATE_PATTERN: std::sync::LazyLock<regex::Regex> = 
      std::sync::LazyLock::new(|| regex::Regex::new(r"-\d{8}$").unwrap());
  ```

**이슈 #22 — Medium: `has_configured_auth()`가 매번 새 `AuthStorage` 생성**
- **위치:** model_resolver.rs:262-265
- **문제:** 함수가 호출될 때마다 `AuthStorage::new()`로 새 인스턴스를 생성합니다. 이는 디스크에서 `auth.json`을 매번 다시 읽습니다.
  ```rust
  pub fn has_configured_auth(provider: &str, _model: &Model) -> bool {
      let auth = crate::auth_storage::AuthStorage::new();  // ← 매번 디스크 읽기
      auth.has_auth(provider)
  }
  ```
- **개선 제안:** `AuthStorage` 인스턴스를 매개변수로 받거나, 싱글톤 패턴 사용.

**이슈 #23 — Low: `get_thinking_level_map()`의 하드코딩된 모델 목록**
- **위치:** model_resolver.rs:282-327
- **문제:** Claude 모델의 thinking level 매핑이 하드코딩되어 있습니다. 새 모델이 추가될 때마다 코드 수정이 필요합니다.
- **개선 제안:** `model_db` 또는 `models.json`에 thinking level 매핑을 정의하고 참조.

---

## 7. 파일 I/O 패턴 (File I/O)

### 7.1 원자적 쓰기

**👍 장점 (Settings):** `settings.rs`의 `save()` / `save_to()` / `save_project()`는 모두 tmp 파일 → rename 패턴을 사용합니다. (settings.rs:652-660)

**이슈 #24 — Critical: Session 파일의 원자적 쓰기 누락**
- **위치:** session.rs:503-517 (`_rewrite_file`), session.rs:462-497 (`_persist`)
- **문제:** 세션 파일은 임시 파일 없이 직접 쓰기를 사용합니다. `fs::write()`와 `writeln!()` 모두 비원자적입니다. 반면 `settings.rs`는 tmp→rename 패턴을 사용합니다. 같은 크레이트 내에서 I/O 패턴이 일관되지 않습니다.
- **개선 제안:** 공통 `atomic_write(path, content)` 유틸리티 함수를 추출하여 모든 파일 쓰기에 사용:
  ```rust
  pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
      let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
      fs::write(&tmp, content)?;
      fs::rename(&tmp, path)?;
      Ok(())
  }
  ```

**이슈 #25 — High: `fork_from()` / `branch_from_entry()`도 비원자적 쓰기**
- **위치:** session.rs:762-782, session.rs:900-925
- **문제:** 새 세션 파일을 `OpenOptions::new().write(true)`로 직접 생성합니다. 쓰기 도중 실패하면 빈/불완전한 파일이 남습니다.
- **개선 제안:** 위와 동일하게 임시 파일 → rename 패턴 사용.

### 7.2 파일 잠금

**이슈 #26 — High: 파일 잠금 메커니즘 부재**
- **위치:** 전체 크레이트
- **문제:** 세션 파일, 설정 파일, auth.json 모두에 파일 잠금이 없습니다. 여러 oxi 프로세스가 같은 세션 파일에 동시에 append하면 데이터가 손상될 수 있습니다.
- **개선 제안:** 
  - `fs2::FileExt::lock_exclusive()` 또는 `flock()` 사용
  - 또는 `.lock` 파일 기반 잠금
  - 세션 파일의 경우 append-only 설계이므로 `O_APPEND` 플래그만으로도 커널 수준 원자성 보장 (Unix)

---

## 8. 데이터 마이그레이션 (Data Migration)

### 8.1 세션 마이그레이션

**파일:** session.rs:385-447

**👍 장점:**
- v1→v2: ID/parent_id 트리 구조 추가
- v2→v3: `hookMessage` → `custom` 역할 변경
- `migrate_to_current_version()`에서 순차적 마이그레이션 보장

**이슈 #27 — Medium: `migrate_v1_to_v2`의 8자리 ID 생성기**
- **위치:** session.rs:370-378
- **문제:** `generate_id()`는 UUID 앞 8자리를 사용하며, 100회 충돌 후에도 전체 UUID로 폴백합니다. 하지만 대규모 세션(수천 개 엔트리)에서는 8자리 ID 충돌 가능성이 높아집니다.
- **개선 제안:** 세션 엔트리 ID에 전체 UUID를 사용하거나, 12-16자리로 늘리기.

**이슈 #28 — Medium: `migrate_v2_to_v3`가 실제로 아무것도 하지 않음**
- **위치:** session.rs:431-445
- **문제:** 헤더 버전만 업데이트하고, 실제 `hookMessage` → `custom` 변환은 수행하지 않습니다. 주석에 "v2 to v3 migration handled elsewhere"라고 되어 있지만, 실제 구현이 없습니다.
  ```rust
  fn migrate_v2_to_v3(entries: &mut Vec<FileEntry>) {
      for entry in entries.iter_mut() {
          match entry {
              FileEntry::Header(header) => { header.version = Some(3); }
              FileEntry::Entry(_) => {
                  // v2 to v3 migration handled elsewhere  ← 비어 있음!
              }
          }
      }
  }
  ```
- **개선 제안:** 실제 변환 로직을 구현하거나, v2→v3 마이그레이션이 필요 없다는 것을 명확히 문서화.

### 8.2 설정 마이그레이션

**파일:** settings.rs:699-753

**👍 장점:** 체계적인 버전별 마이그레이션 경로

**이슈 #29 — Low: v1→v4 점프 마이그레이션의 누락된 단계**
- **위치:** settings.rs:724-730
- **문제:** v1/2에서 v4로 직접 점프합니다. 중간 버전(v3의 `provider/model` 분리)을 건너뜁니다. 다행히 v3 마이그레이션은 `default_model`에 슬래시가 있을 때만 작동하므로 실제 문제는 없을 수 있지만, 명시적으로 단계를 밟는 것이 안전합니다.
- **개선 제안:** `if version < 3 { migrate_v1_to_v3(...) }` 등 순차적 적용.

---

## 9. 세션 내비게이션 (Session Navigation)

**파일:** `session_navigation.rs`

### 9.1 아키텍처

`SessionNavigator`는 세션 트리 내의 내비게이션을 관리합니다:
- 브랜치 탐색 (`get_branch()`, `get_children()`)
- 브랜치 요약 수집 (`collect_entries_for_branch_summary()`)
- 내비게이션 + 요약 생성 (`navigate_tree()`)
- 확장 훅 통합 (`BeforeTreeHookResult`)

**👍 장점:**
- `Summarizer` trait을 통한 테스트 가능한 설계
- 확장 훅(`extension_hook`)으로 커스터마이징 가능
- `NavigationOptions`으로 요약 생성 제어
- 포괄적인 테스트 커버리지 (30+ 테스트)

**이슈 #30 — High: `navigate_tree()`에서 `tokio::runtime::Handle::current()` 블로킹**
- **위치:** session_navigation.rs:326-329
- **문제:** 동기 함수 내에서 `block_on()`을 호출합니다. 이는 tokio 런타임 컨텍스트 내에서 호출되면 패닉을 일으킬 수 있습니다 (tokio는 같은 런타임 내에서 `block_on`을 금지).
  ```rust
  let rt = summarizer.summarize(/* ... */);
  let runtime = tokio::runtime::Handle::current();
  let result = runtime.block_on(rt);  // ← tokio 컨텍스트에서 패닉 가능
  ```
- **개선 제안:** `navigate_tree()`를 `async fn`으로 변경하거나, `spawn_blocking`을 사용하여 별도 스레드에서 실행.

**이슈 #31 — Medium: `SessionNavigator`가 `SessionManager`와 별개의 타입 시스템**
- **위치:** session_navigation.rs vs session.rs
- **문제:** `session_navigation.rs`의 `SessionEntryType`, `MessageEntry`, `MessageRole`이 `session.rs`의 `SessionEntry`, `AgentMessage`와 별개입니다. 두 모듈 간 변환 함수가 없어 데이터 중복과 불일치 위험이 있습니다.
- **개선 제안:** 공통 타입을 하나로 통합하거나, 명시적 변환 함수(try-from)를 제공.

---

## 10. 동시성 안전성 (Concurrency Safety)

### 10.1 락 사용 패턴

**파일:** session.rs (여러 위치)

`SessionManager`는 `parking_lot::RwLock`을 사용합니다:
- `persisted_count: RwLock<usize>`
- `file_entries: RwLock<Vec<FileEntry>>`
- `by_id: RwLock<HashMap<String, SessionEntry>>`
- `labels_by_id: RwLock<HashMap<String, String>>`
- `label_timestamps_by_id: RwLock<HashMap<String, String>>`
- `leaf_id: RwLock<Option<String>>`

**👍 장점:**
- `parking_lot::RwLock`은 `std::sync::RwLock`보다 성능이 좋고 데드락에 강함
- 읽기 작업이 많은 워크로드에 적합

**이슈 #32 — Critical: 다중 락 획득 순서로 인한 데드락 가능성**
- **위치:** session.rs:540-549 (`_append_entry`)
- **문제:** `_append_entry()`는 `file_entries.write()` → `by_id.write()` → `leaf_id.write()`를 순차적으로 획득합니다. 다른 메서드(예: `_build_index()`)는 다른 순서로 락을 획득할 수 있어 데드락 위험이 있습니다.
  ```rust
  fn _append_entry(&mut self, entry: SessionEntry) {
      let file_entry = convert_from_session_entry(&entry);
      self.file_entries.write().push(FileEntry::Entry(file_entry));  // Lock 1
      self.by_id.write().insert(entry.id.clone(), entry.clone());   // Lock 2
      *self.leaf_id.write() = Some(entry.id.clone());               // Lock 3
      self._persist(&entry);  // 내부에서 file_entries.read() 획득 (Lock 1 재획득)
  }
  ```
- **개선 제안:** 
  1. 모든 상태를 단일 `RwLock<SessionState>` 구조체로 통합
  2. 또는 항상 동일한 순서(`file_entries` → `by_id` → `labels_by_id` → `leaf_id`)로만 락 획득
  3. `_persist()` 호출 전에 모든 write 락을 해제

**이슈 #33 — High: `&mut self`와 `RwLock`의 혼용**
- **위치:** session.rs (여러 메서드)
- **문제:** `_persist()`, `_append_entry()`, `_build_index()` 등은 `&mut self`를 받습니다. `parking_lot::RwLock`은 내부 가변성을 제공하므로 `&self`로 충분합니다. `&mut self`는 외부에서 단일 스레드 접근을 보장받지만, `RwLock`은 다중 스레드 접근을 허용하는 설계입니다. 두 접근 방식이 혼재되어 있어 혼란스럽습니다.
- **개선 제안:** 모든 락 필드 접근을 `&self`로 통일하고 `&mut self` 메서드를 제거.

**이슈 #34 — Medium: `get_branch()`의 O(n) 경로 탐색**
- **위치:** session.rs:613-624
- **문제:** `get_branch()`가 `parent_id`를 따라 올라가며 각 단계에서 `by_id.read()`를 호출합니다. 긴 세션(수천 엔트리)에서 성능이 저하될 수 있습니다.
  ```rust
  while let Some(entry) = current {
      path.insert(0, entry.clone());
      current = entry.parent_id.as_ref()
          .and_then(|pid| self.by_id.read().get(pid).cloned());  // ← 매 단계마다 락
  }
  ```
- **개선 제안:** `by_id.read()`를 한 번만 획득하고, 그 안에서 전체 경로를 탐색:
  ```rust
  let by_id = self.by_id.read();
  while let Some(entry) = current {
      path.insert(0, entry.clone());
      current = entry.parent_id.as_ref()
          .and_then(|pid| by_id.get(pid).cloned());
  }
  ```

---

## 11. CWD 추적 (CWD Tracking)

**파일:** `session_cwd.rs`, `session.rs` (SessionHeader)

### 11.1 설계

`session_cwd.rs`는 세션의 작업 디렉토리가 존재하는지 확인하는 유틸리티를 제공합니다.

**👍 장점:**
- `SessionCwdSource` trait으로 테스트 가능한 설계
- `assert_session_cwd_exists()`로 명확한 에러 타입 (`MissingSessionCwdError`)
- 포괄적인 테스트 (4개)

**이슈 #35 — Medium: CWD가 세션 헤더에만 저장됨**
- **위치:** session.rs:129-144 (`SessionHeader`)
- **문제:** CWD는 세션 생성 시 한 번만 저장됩니다. 사용자가 세션 중간에 다른 디렉토리로 `cd`하면 세션 파일의 CWD와 실제 CWD가 불일치합니다.
- **개선 제안:** CWD 변경 이벤트를 세션 엔트리로 기록하거나, 세션 재개 시 항상 현재 CWD를 사용.

**이슈 #36 — Low: `format_missing_session_cwd_error`의 이스케이프 오류**
- **위치:** session_cwd.rs:55
- **문제:** `"\\n"`이 리터럴 백슬래시-n으로 포맷됩니다. `format!()` 매크로에서 `\n`은 이미 개행 문자이므로 이중 이스케이프입니다.
  ```rust
  format!(
      "Stored session working directory does not exist: {}{}\\nCurrent working directory: {}",
      issue.session_cwd, session_file_line, issue.fallback_cwd
  )
  ```
- **개선 제안:** `"\\n"` → `"\n"`으로 수정.

---

## 12. 종합 평가

### 12.1 강점

1. **체계적인 모듈 구조:** 세션, 설정, 인증, 모델 레지스트리가 명확히 분리
2. **포괄적인 테스트:** 각 모듈에 20-30개 이상의 단위 테스트
3. **JSON/TOML 듀얼 포맷:** 설정 파일의 유연성
4. **마이그레이션 인프라:** 버전 기반 설정/세션 마이그레이션
5. **확장성:** `AuthStorageBackend` trait, `Summarizer` trait, 동적 프로바이더 등록
6. **인증 안내:** `auth_guidance.rs`의 프로바이더별 맞춤 메시지

### 12.2 최우선 개선 항목

| 우선순위 | 이슈 | 심각도 | 설명 |
|----------|------|--------|------|
| 1 | #24 | Critical | 세션 파일 원자적 쓰기 |
| 2 | #12 | Critical | API 키 평문 저장 |
| 3 | #32 | Critical | 다중 락 데드락 위험 |
| 4 | #18 | High | models.json 명령어 주입 |
| 5 | #1 | High | _rewrite_file 비원자적 |
| 6 | #26 | High | 파일 잠금 메커니즘 부재 |
| 7 | #30 | High | navigate_tree block_on 패닉 |
| 8 | #13 | High | persist() 에러 무시 |

### 12.3 아키텍처 개선 제안

1. **공통 I/O 유틸리티:** `atomic_write()`, `safe_append()` 함수를 별도 모듈로 추출
2. **상태 통합:** `SessionManager`의 6개 `RwLock`을 `RwLock<SessionState>`로 통합
3. **인증 암호화:** keyring을 기본으로, 파일 저장 시 암호화
4. **비동기 일관성:** `navigate_tree()`를 async로 변경하거나, 별도 스레드에서 실행
5. **타입 통합:** `session.rs`와 `session_navigation.rs`의 중복 타입을 단일 타입 계층으로 통합

---

*이 보고서는 정적 코드 분석을 기반으로 작성되었습니다. 동적 분석(프로파일링, 퍼징 등)은 포함되지 않았습니다.*

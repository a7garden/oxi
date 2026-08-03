# 커스텀 프로바이더 등록 가이드

> **상태: 초안 (기능 미구현)**
>
> 이 문서는 커스텀 프로바이더 등록 기능이 구현된 후 사용할 가이드입니다.
> 현재 `[[custom_provider]]` 설정은 지원되지 않습니다.

OpenAI 호환 API를 사용하는 서드파티 프로바이더를 oxicode에 등록할 수 있습니다.
등록된 프로바이더는 빌트인 프로바이더와 동일하게 `provider/model` 형식으로 사용할 수 있습니다.

---

## 설정

`~/.oxicode/settings.toml`에 `[[custom_provider]]` 섹션을 추가합니다:

```toml
# ── Minimax ──────────────────────────────────────────────────────────
[[custom_provider]]
name = "minimax"
base_url = "https://api.minimax.chat/v1"
api_key_env = "MINIMAX_API_KEY"
# OpenAI 호환 API 타입 (기본값: openai_completions)
api = "openai_completions"

# ── ZAI (ZhipuAI) ───────────────────────────────────────────────────
[[custom_provider]]
name = "zai"
base_url = "https://api.z.ai/v1"
api_key_env = "ZAI_API_KEY"
api = "openai_completions"
```

### 설정 필드

| 필드 | 필수 | 설명 |
|------|------|------|
| `name` | ✅ | 프로바이더 고유 식별자. 명령어에서 `name/model` 형태로 사용 |
| `base_url` | ✅ | API 엔드포인트 베이스 URL |
| `api_key_env` | ✅ | API 키를 읽어올 환경 변수 이름 |
| `api` | ❌ | API 타입 (기본값: `openai_completions`) |

### 지원 API 타입

| 값 | 설명 |
|----|------|
| `openai_completions` | OpenAI Chat Completions 호환 (기본값) |
| `openai_responses` | OpenAI Responses API 호환 |

---

## API 키 설정

API 키는 환경 변수로 관리합니다. 쉘 설정 파일(`~/.bashrc`, `~/.zshrc` 등)에 추가:

```bash
export MINIMAX_API_KEY="your-minimax-api-key"
export ZAI_API_KEY="your-zai-api-key"
```

또는 세션별로 임시 설정:

```bash
# 현재 세션에만 적용
export MINIMAX_API_KEY="your-key"
oxicode -m minimax/MiniMax-M1 "Hello"
```

---

## 사용법

### 기본 사용

```bash
# Minimax 모델 사용
oxicode -m minimax/MiniMax-M1 "Hello, world!"

# ZAI 모델 사용
oxicode -m zai/glm-4 "안녕하세요"

# 모델 이름은 프로바이더 API가 지원하는 모델 ID를 그대로 사용
oxicode -m minimax/MiniMax-Text-01 "Explain quantum computing"
oxicode -m zai/glm-4-flash "Quick summary"
```

### 기본 모델로 설정

`~/.oxicode/settings.toml`에서 기본 모델로 지정:

```toml
default_model = "minimax/MiniMax-M1"
```

또는 환경 변수:

```bash
export OXICODE_MODEL="minimax/MiniMax-M1"
```

---

## 등록 예시: 추가 프로바이더

### Together AI

```toml
[[custom_provider]]
name = "together"
base_url = "https://api.together.xyz/v1"
api_key_env = "TOGETHER_API_KEY"
```

```bash
export TOGETHER_API_KEY="your-key"
oxicode -m together/meta-llama/Llama-3.3-70B-Instruct-Turbo "Hello"
```

### Grok (직접 등록)

```toml
[[custom_provider]]
name = "grok"
base_url = "https://api.x.ai/v1"
api_key_env = "XAI_API_KEY"
```

```bash
export XAI_API_KEY="your-key"
oxicode -m grok/grok-2 "Hello"
```

### 로컬 모델 (Ollama 등)

```toml
[[custom_provider]]
name = "ollama"
base_url = "http://localhost:11434/v1"
api_key_env = "OLLAMA_API_KEY"  # Ollama는 키가 필요 없지만 필수 필드
```

```bash
# Ollama는 API 키가 필요 없으므로 빈 값 설정
export OLLAMA_API_KEY="ollama"
oxicode -m ollama/llama3 "Hello"
```

---

## 문제 해결

### "Provider not found" 에러

- `name` 필드가 명령어의 프로바이더 부분과 정확히 일치하는지 확인
- `settings.toml` 파일이 `~/.oxicode/` 또는 `.oxicode/` 디렉토리에 있는지 확인

### "API key not found" 에러

- `api_key_env`에 지정한 환경 변수가 설정되어 있는지 확인:
  ```bash
  echo $MINIMAX_API_KEY
  ```

### 연결 에러

- `base_url`이 올바른지 확인 (경로에 `/v1` 포함 여부 등)
- API 서비스가 정상인지 `curl`로 테스트:
  ```bash
  curl https://api.minimax.chat/v1/models \
    -H "Authorization: Bearer $MINIMAX_API_KEY"
  ```

---

## 제한 사항

- OpenAI 호환 API만 지원합니다 (Anthropic, Google 등 타 API는 지원 불가)
- 스트리밍 응답은 SSE(Server-Sent Events) 형식이어야 합니다
- 커스텀 프로바이더는 빌트인 프로바이더 레지스트리에 등록되지 않습니다
  (별도의 커스텀 프로바이더 경로로 처리됩니다)

---

## 관련 문서

- [oxicode 설정 가이드](../oxicode-cli/src/settings.rs) — settings.toml 구조
- [프로바이더 레지스트리](../oxicode-ai/src/provider_registry.rs) — 인증 관리
- [빌트인 프로바이더](../oxicode-ai/src/providers/register_builtins.rs) — 기본 프로바이더 목록

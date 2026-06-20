# 세부 설계 ① — Hashline line-anchored edit

> 상태: 설계 v1 (구현 전 합의용)
> 작성: 2026-06-19
> 선행: [`00-master-plan.md`](./00-master-plan.md), omp `packages/hashline/` v16.1.1 심층 분석
> 후속: M1 구현 → CHANGELOG.md + AGENTS.md "Adding a New Edit Format" 섹션 + 시스템 프롬프트 갱신
> **본 문서는 omp-adoption 로드맵의 핵심이다.** ②/③/④가 의존하거나 병렬로 진행되며, 가장 큰 가치와 가장 큰 리팩토링을 수반한다.

---

## 0. 핵심 (TL;DR)

omp의 **Hashline**은 str_replace 대신 **행 번호 anchor + 콘텐츠 해시 tag**로 편집한다. 모델이 `read`로 본 파일의 정확한 행을 가리켜 변경하므로, "문자열 못 찾음 → 재시도 루프 → 토큰 폭발" 고질병이 사라진다. omp 실측: **Grok 4 Fast 토큰 −61%, MiniMax pass rate 2.1×**.

본 설계는 omp의 `packages/hashline/` (4.7K LOC)를 **`oxi-hashline` 독립 Rust 크레이트**로 이식하고, 기존 `edit.rs`(str_replace)와 **공존**시킨다.

### 5개 핵심 메커니즘 (omp에서 그대로 가져올 것)

| # | 메커니즘 | omp 위치 | 해결하는 문제 |
|:-:|---|---|---|
| 1 | **4-hex content tag** (xxHash32 하위 16-bit) | `format.ts:computeFileHash` | 불안정한 해시 → 크로스플랫폼 안정 tag |
| 2 | **SnapshotStore** (per-path 버전 히스토리 + seenLines) | `snapshots.ts` | 드리프트 복구 + "안 본 줄" 편집 차단 |
| 3 | **3-way merge 복구** + session chain replay | `recovery.ts` | 파일 변경 시 거부만 → 자동 복구 |
| 4 | **boundary repair** (5가지 패턴) | `apply.ts:repairReplacementBoundaries` | 모델의 범위 경계 실수 자동 교정 |
| 5 | **after-insert landing correction** | `apply.ts:resolveShiftedLanding` | 들여쓰기 기반 insert 위치 보정 |

### oxi에 주는 가치 (정량)

- **토큰 절약**: str_replace는 `oldText` 전문을 재입력해야 함. Hashline은 행 번호 + `+TEXT`만. 큰 블록 교체 시 차이 벌어짐.
- **재시도 감소**: tag가 발산하면 즉시 거부 → 모델이 `re-read` 후 재시도 (1회). str_replace는 부분 매칭 실패 시 모델이 추측하며 여러 번 재시도.
- **안전성**: `seenLines` 검증으로 모델이 `read`로 본 적 없는 줄(기억 착오)을 편집해 파일을 훼손하는 것 차단.
- **all-or-nothing**: 멀티섹션 패치는 모든 섹션 preflight 후 일괄 커밋. 부분 실패 시 어떤 섹션이 적용됐는지 보고.

---

## 1. 배경: omp Hashline이 해결하는 5가지 str_replace 한계

### 1.1 oxi의 현재 edit (str_replace)

```
read src/foo.rs          →  "Showing lines 1-50 of 120:" + "{linenum}\t{content}"
edit (path, oldText, newText)  →  oldText를 파일에서 찾아 newText로 교체
                                expected_hash (DefaultHasher 64-bit)로 충돌 감지
```

파일: `oxi-agent/src/tools/edit.rs`, `oxi-agent/src/tools/edit_diff.rs`.

### 1.2 한계 ① — 불안정한 해시

```rust
// edit.rs 현재
use std::hash::{Hash, Hasher};
let mut hasher = std::collections::hash_map::DefaultHasher::new();
current_content.hash(&mut hasher);
let current_hash = format!("{:016x}", hasher.finish());
```

`DefaultHasher`는 **SipHash-1-3**이지만 시드가 프로세스마다 **무작위화되지는 않음**에도 Rust 버전/아키텍처에 따라 결과가 달라질 수 있고, 공식적으로 "안정적이지 않음"을 명시. 세션 지속·멀티 프로세스·CI 재현성에서 tag 발산 위험.

omp는 `Bun.hash.xxHash32(normalized) & 0xffff` → **4-hex, 크로스플랫폼 결정론적**. 어떤 언어/런타임에서 읽어도 같은 tag.

### 1.3 한계 ② — 드리프트 시 거부만

oxi는 `expected_hash` 불일치 시 `"File has been modified since last read. Re-read the file and retry."` 만 응답. 모델이 매번 재시도해야 함.

omp는 **2단계 복구**:
1. tag가 이름붙인 스냅샷 버전에 edit를 적용 → 그 diff를 live content에 3-way merge (`Diff.applyPatch`, fuzz 0).
2. (session chain) 이전 in-session edit가 tag를 진행시킨 경우 — line count 동일 + anchor 행 내용 동일하면 live에 직접 replay.

실패 시에만 `MismatchError`(re-read). omp의 recovery는 모델에게 투명하게 작동.

### 1.4 한계 ③ — str_replace 고질병 (토큰/정확도)

- `oldText`가 파일에 **유일하지 않으면** 매칭 실패 → 모델이 더 많은 문맥을 붙여 재시도 → 토큰 증가.
- `oldText`에 **공백/들여쓰기 차이** → 매칭 실패.
- 큰 블록 교체 시 `oldText` 전문 재입력 → 출력 토큰 폭발.

Hashline은 행 번호로 가리키므로 유일성/공백 문제가 없고, body는 **최종 내용만** (`+TEXT`).

### 1.5 한계 ④ — "안 본 줄" 편집 허용

oxi는 모델이 `read`로 본 적 없는 줄 번호를 anchor로 써도 (기억 착오) 적용 시도 → 파일 훼손.

omp는 `SnapshotStore`가 각 tag에 `seenLines: Set<number>`를 기록. patcher가 `#assertSeenLines`로 anchor가 본 줄인지 검증 — **안 본 줄이면 즉시 거부 + "re-read those exact lines"**.

### 1.6 한계 5 — 모델 실수 자동 교정 없음

str_replace는 모델이 `oldText`를 정확히 재현해야만 성공. Hashline의 `repairReplacementBoundaries`는 흔한 실수 5패턴을 **경고와 함께 자동 교정**:
1. **boundary echo** — payload가 범위 밖 줄을 그대로 재입력 (양쪽).
2. **one-sided echo** — 한쪽만 재입력.
3. **duplicate suffix/prefix** — 구조 closer 중복.
4. **dropped suffix closers** — 범위가 closer를 삭제했는데 payload가 안 쓰는 경우 closer 보존.
5. **delimiter balance** — `()`/`[]`/`{}` 균형 추적으로 교정 여부 결정 (+ JSX 태그 균형).

→ 모델이 "거의 맞게" 쓴 패치를 살려내어 재시도를 줄임.

---

## 2. omp 메커니즘 심층 (포팅 명세)

### 2.1 문법 (grammar.lark)

```
patch: begin_patch file_patch+ end_patch
file_patch: "[" filename "#" file_hash "]" LF hunk+
file_hash: /[0-9A-F]{4}/
hunk: replace_hunk | insert_hunk | delete_hunk
     ( + replace_block_hunk | insert_block_hunk | delete_block_hunk — **후순위 확장**, tree-sitter 필요; 본 로드맵 M1은 라인 op만)
replace_hunk: "SWAP " start ".=" end ":" LF emit_op+
insert_hunk: "INS." ("PRE " LID | "POST " LID | "HEAD" | "TAIL") ":" LF emit_op+
delete_hunk: "DEL " (start ".=" end | LID) LF
emit_op: "+" /(.*)/ LF
```

**핵심 제약** (prompt.md):
- 범위는 **원본 행 번호**, 적용 중 이동 안 함.
- body는 **최종 내용만** (`+TEXT`), `-old` 행 없음.
- `[PATH#TAG]`의 TAG는 **최신 read/search에서 발행한 것**, 모든 섹션에 필수.
- 모든 적용은 **새 #TAG 발행 + 재번호** → 다음 edit는 응답이나 re-read에서 번호를 가져온다.

### 2.2 content tag (format.ts)

```ts
function normalizeFileHashText(text) { return text.replace(/[ \t\r]+(?=\n|$)/g, ""); }
function computeFileHash(text) {
  const normalized = normalizeFileHashText(text);
  const low16 = Bun.hash.xxHash32(normalized, 0) & 0xffff;
  return low16.toString(16).padStart(4, "0").toUpperCase();
}
```

- trailing `[ \t\r]` 제거 (CRLF/표시-trim에 강건).
- xxHash32 시드 0, 하위 16-bit → 4-hex 대문자.

### 2.3 SnapshotStore (snapshots.ts)

```
Snapshot { path, text(정규화), hash, recordedAt, seenLines?: Set<u32> }
SnapshotStore trait:
  head(path) -> Option<Snapshot>
  by_hash(path, hash) -> Option<Snapshot>
  record(path, full_text, seen_lines?) -> hash
  record_seen_lines(path, hash, lines)
  invalidate(path) / clear()
```

`InMemorySnapshotStore`: LRU 기반 (기본: 경로 30개 × 버전 4개 × 총 64MiB). 같은 내용 재기록 시 recency 갱신 + tag 재사용 (read fusion). `seenLines`는 동일 내용 재읽기 시 union.

### 2.4 Patcher (patcher.ts) — 2단계 커밋

```
prepare(section): read → BOM/CRLF 정규화 → tag 검증(recovery 포함) → 메모리 apply
commit(prepared): line-ending/BOM 복원 → write → 새 snapshot 기록
apply(patch): 모든 섹션 prepare 후 일괄 commit (all-or-nothing)
preflight(patch): prepare만, write 없음 (CI/dry-run)
```

**드리프트 처리 결정 트리** (`#applyWithRecovery`):
```
expected tag 있고 live == tag  →  assertSeenLines + applyEdits (정상 경로)
tag 없음                       →  applyEdits (tag 없이)
head/tail insert만 있음         →  applyEdits + HEADTAIL_DRIFT_WARNING
그 외 (anchored edit + 드리프트) →  recovery.tryRecover:
                                   ① [M1] session chain: line count + anchor content 가드 → live replay
                                   ② [M1.5] 3-way merge: tag 버전에 apply → diff → live에 적용 (similar)
                                   실패 → MismatchError (re-read)
```
> recovery ①은 M1에, ②는 post-M1(M1.5)에 구현된다 (§3.7 참조). M1에서 외부
> 수정 케이스(is_head + live ≠ tag)는 MismatchError로 폴백한다 — str_replace 현재
> 동작과 동등하며, 대다수 드리프트는 session chain으로 처리된다.

### 2.5 boundary repair (apply.ts) — 모델 실수 5패턴

`repairReplacementBoundaries(edits, file_lines)`:
1. `findBoundaryEcho` — payload 앞/뒤가 범위 밖 줄과 동일 (양쪽) → 중복 행 drop.
2. `findOneSidedBoundaryEcho` — 한쪽만 동일 + delimiter 균형 0 → drop.
3. `findDuplicateSuffix/Prefix` — delimiter 균형이 깨졌는데 중복 행이 있으면 drop.
4. `findDroppedSuffixClosers` — closer가 삭제됐는데 payload가 안 쓰면 closer 보존.
5. `computeDelimiterBalance` — `()`/`[]`/`{}` 카운트 (문자열/주석/템플릿 리터럴 스킵) + JSX 태그 균형 (`readJsxPayloadTags`).

모든 교정은 **경고 메시지** 동반 (모델에게 피드백).

### 2.6 after-insert landing correction (apply.ts)

`INS.POST N:` body의 들여쓰기가 N행보다 **얕으면** → N 아래의 구조 closer 줄들을 지나 sibling 위치로 이동 (`resolveShiftedLanding`). `INS.BLK.POST N:`(**후순위 확장**)은 반대 방향(블록 안쪽). 보수적: 들여쓰기가 prefix 관계일 때만, closer 줄만 건넘, 다른 edit가 건드리는 줄이면 포기.

### 2.7 all-or-nothing + 부분 실패 보고

멀티섹션 패치: 모든 섹션 `prepare` 성공해야 commit 시작. mid-batch write 실패 시: "이미 적용된 섹션 / 미적용 섹션" 명시 → 모델이 누락분만 재발행.

---

## 3. oxi화 설계 — `oxi-hashline` 크레이트

### 3.1 크레이트 위치와 의존

**새 크레이트**: `oxi-hashline/` (워크스페이스 루트, `oxi-ai`와 동급).

**의존성 흐름**:
```
oxi-ai  ←  oxi-agent  ←  oxi-sdk  ←  oxi-cli
oxi-hashline  (독립, oxi-* 의존 없음 — 순수 함수 라이브러리)
              ↑
              oxi-agent 의존 (edit 도구가 사용)
```

`Cargo.toml`:
```toml
[package]
name = "oxi-hashline"
version = {workspace}
edition = "2024"
rust-version = {workspace}
license = "MIT"

[dependencies]
xxhash-rust = { version = "0.8", features = ["xxh32"] }   # compute_file_hash
lru = "0.12"                                                # InMemorySnapshotStore
thiserror = {workspace}                                     # HashlineError
serde = { version = "1", features = ["derive"] }            # 직렬화(선택)

[dependencies.similar]                                        # 3-way merge recovery (M1.5). M1에서는 불필요.
version = "2"
optional = true

[features]
default = []
block-ops = ["dep:tree-sitter"]                             # SWAP.BLK/DEL.BLK/INS.BLK.POST
three-way-merge = ["dep:similar"]                           # recovery Phase 2 (M1.5)
```

> **설계 결정**: `block-ops`(tree-sitter 기반 `SWAP.BLK` 등)는 **후순위 확장**이다. 본 로드맵 M1은 `default` feature만 — 라인 op(`SWAP/DEL/INS.PRE/POST/HEAD/TAIL`)로 omp 가치의 ~80%를 달성한다. tree-sitter는 무거운 의존(언어별 1-2MB + C 코드 + 빌드 시간)이므로, block-ops가 필요해지는 시점에 별도 도입한다. AGENTS.md의 "native-browser feature" 패턴(기본 빌드에 무거운 의존 추가 안 함)과 동일.

### 3.2 모듈 구조 (omp 1:1 대응)

```
oxi-hashline/src/
├── lib.rs          (재진입점 + 공개 API re-export)
├── format.rs       ← omp format.ts        (sigil 상수, compute_file_hash, format_*)
├── grammar.rs      ← omp grammar.lark     (토큰/키워드 — Rust enum/const)
├── types.rs        ← omp types.ts         (Edit, Anchor, Cursor, ApplyResult, BlockSpan)
├── tokenizer.rs    ← omp tokenizer.ts     (LID/range/헤더 라인 토큰화)
├── parser.rs       ← omp parser.ts+input.ts (PatchSection::parse, split_patch_input)
├── normalize.rs    ← omp normalize.ts     (BOM/CRLF — oxi edit_diff.rs 함수 이전)
├── snapshots.rs    ← omp snapshots.ts     (SnapshotStore trait + InMemorySnapshotStore)
├── apply.rs        ← omp apply.ts         (apply_edits, repair_replacement_boundaries)
├── recovery.rs     ← omp recovery.ts      (M1: session chain; M1.5: 3-way merge)
├── block.rs        ← omp block.ts         (resolve_block_edits, BlockResolver seam) [block-ops]
├── patcher.rs      ← omp patcher.ts       (Patcher: prepare/commit/preflight)
├── mismatch.rs     ← omp mismatch.ts      (MismatchError + 진단)
├── messages.rs     ← omp messages.ts      (사용자 메시지 템플릿 — 한국어/영어)
├── diff_preview.rs ← omp diff-preview.ts  (CompactDiffPreview)
├── stream.rs       ← omp stream.ts        (stream_hash_lines — 스트리밍 미리보기)
└── prompt.md       ← omp prompt.md        (모델용 문법 명세 — oxi 컨텍스트로 번역)
```

### 3.3 핵심 타입 (types.rs)

```rust
//! omp types.ts의 Rust 이식. FS/런타임/스키마 라이브러리 의존 없음 — 순수 데이터.

/// 1-indexed 행 anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor { pub line: u32 }

/// insert가 착지할 위치.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    Bof,                                  // INS.HEAD
    Eof,                                  // INS.TAIL
    BeforeAnchor(Anchor),                 // INS.PRE N
    AfterAnchor(Anchor),                  // INS.POST N
}

/// parser → applier로 흐르는 저수준 edit.
#[derive(Debug, Clone)]
pub enum Edit {
    Insert {
        cursor: Cursor,
        text: String,
        line_num: u32,                    // 패치 내 위치 (진단용)
        index: usize,
        mode: Option<InsertMode>,         // Replacement = SWAP에서 lowering
        block_start: Option<u32>,         // INS.BLK.POST lowering (block-ops)
    },
    Delete {
        anchor: Anchor,
        line_num: u32,
        index: usize,
        old_assertion: Option<String>,
    },
    Block {                               // [block-ops] SWAP.BLK/DEL.BLK/INS.BLK.POST
        anchor: Anchor,
        payloads: Vec<String>,
        mode: Option<BlockMode>,
        line_num: u32,
        index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertMode { Replacement }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMode { InsertAfter }

#[derive(Debug, Clone, Default)]
pub struct ApplyResult {
    pub text: String,
    pub first_changed_line: Option<u32>,
    pub warnings: Vec<String>,
    pub block_resolutions: Vec<BlockResolution>,   // [block-ops]
}

#[derive(Debug, Clone)]
pub struct BlockResolution {                         // [block-ops]
    pub anchor_line: u32,
    pub start: u32,
    pub end: u32,
    pub op: BlockOp,
}
#[derive(Debug, Clone, Copy)]
pub enum BlockOp { Replace, Delete, InsertAfter }

/// tree-sitter 블록 해석 seam (host가 주입). [block-ops]
pub type BlockResolver = Arc<dyn Fn(BlockResolverRequest) -> Option<BlockSpan> + Send + Sync>;
```

### 3.4 format.rs — 단일 진실 소스

```rust
//! omp format.ts 이식. prompt.md와 코드가 같은 상수를 참조한다.

pub const HL_FILE_PREFIX: &str = "[";
pub const HL_FILE_SUFFIX: &str = "]";
pub const HL_FILE_HASH_SEP: char = '#';
pub const HL_RANGE_SEP: &str = ".=";
pub const HL_LINE_BODY_SEP: char = ':';
pub const HL_PAYLOAD_REPLACE: char = '+';
pub const HL_HEADER_COLON: char = ':';
pub const HL_FILE_HASH_LENGTH: usize = 4;

pub const HL_REPLACE_KEYWORD: &str = "SWAP";
pub const HL_DELETE_KEYWORD: &str = "DEL";
pub const HL_INSERT_KEYWORD: &str = "INS";
pub const HL_INSERT_BEFORE: &str = "PRE";
pub const HL_INSERT_AFTER: &str = "POST";
pub const HL_INSERT_HEAD: &str = "HEAD";
pub const HL_INSERT_TAIL: &str = "TAIL";
// [block-ops] HL_REPLACE_BLOCK_KEYWORD = "SWAP.BLK" 등

/// trailing [ \t\r] 제거 — CRLF/표시-trim에 강건.
fn normalize_file_hash_text(text: &str) -> String {
    // omp: text.replace(/[ \t\r]+(?=\n|$)/g, "")
    // Rust: regex 또는 수동 스캔 (의존 최소화 위해 수동 권장)
    ...
}

/// xxHash32 시드 0, 하위 16-bit → 4-hex 대문자. omp computeFileHash와 동일 결과.
pub fn compute_file_hash(text: &str) -> String {
    let normalized = normalize_file_hash_text(text);
    let low16 = xxhash_rust::xxh32::xxh32(normalized.as_bytes(), 0) & 0xFFFF;
    format!("{:04X}", low16)
}

pub fn format_hashline_header(path: &str, hash: &str) -> String { format!("[{}{}{}]", path, HL_FILE_HASH_SEP, hash) }
pub fn format_numbered_line(line: u32, text: &str) -> String { format!("{}{}{}", line, HL_LINE_BODY_SEP, text) }
```

> **검증 필수**: `compute_file_hash`는 omp와 **byte-identical** 결과를 내야 한다. M1 첫 태스크로 omp 테스트 벡터를 Rust 테스트로 이식 — `assert_eq!(compute_file_hash("..."), "1A2B")`.

### 3.5 snapshots.rs — LRU 버전 히스토리

```rust
//! omp snapshots.ts 이식. lru 크레이트 기반.

pub struct Snapshot {
    pub path: String,
    pub text: String,                     // 정규화(LF, BOM 제거)
    pub hash: String,
    pub recorded_at: SystemTime,
    pub seen_lines: Option<HashSet<u32>>, // read/search가 실제 표시한 1-indexed 행
}

pub trait SnapshotStore: Send + Sync {
    fn head(&self, path: &str) -> Option<Snapshot>;
    fn by_hash(&self, path: &str, hash: &str) -> Option<Snapshot>;
    fn record(&self, path: &str, full_text: &str, seen_lines: Option<&[u32]>) -> String;
    fn record_seen_lines(&self, path: &str, hash: &str, lines: &[u32]);
    fn invalidate(&self, path: &str);
    fn clear(&self);
}

pub struct InMemorySnapshotStore { /* Arc<RwLock<LruCache<String, Vec<Snapshot>>>> */ }

impl InMemorySnapshotStore {
    pub fn new() -> Self { Self::with_options(Options::default()) }     // 30 paths × 4 versions × 64MiB
    pub fn with_options(opts: Options) -> Self { ... }
}
```

> **주의 (AGENTS.md pitfall)**: `parking_lot::MutexGuard`는 `!Send` → `.await` 전에 drop. SnapshotStore 메서드는 동기(메모리 연산만)이므로 `async` 불필요 — omp도 동기. `lru::LruCache`는 `!Sync`이므로 `Arc<RwLock<>>` 또는 `Arc<Mutex<>>`로 감쌈. 읽기 다수/쓰기 소수이므로 `parking_lot::RwLock`.

### 3.6 apply.rs — `apply_edits` + boundary repair

```rust
//! omp apply.ts 이식. 순수 함수 (FS 없음, 입력 변형 없음).

/// parser/edit list를 텍스트에 적용. omp applyEdits.
pub fn apply_edits(text: &str, edits: &[Edit]) -> ApplyResult {
    let mut file_lines: Vec<String> = text.split('\n').map(String::from).collect();
    let mut origins = vec![LineOrigin::Original; file_lines.len()];
    drop_trailing_phantom_deletes(&mut applied, &file_lines);
    validate_line_bounds(&applied, &file_lines)?;
    // 1. replacement 그룹 boundary repair (5 패턴)
    let (applied, repair_warnings) = repair_replacement_boundaries(applied, &file_lines);
    // 2. after-insert landing correction
    let (applied, landing_warnings) = correct_after_insert_landings(applied, &file_lines);
    // 3. 행별 버킷팅 + splice 적용
    apply_bucketed(&mut file_lines, &mut origins, &applied);
    ApplyResult {
        text: file_lines.join("\n"),
        first_changed_line: find_first_changed(text, &file_lines),
        warnings: { let mut w = repair_warnings; w.extend(landing_warnings); w },
        block_resolutions: vec![],
    }
}
```

**boundary repair 서브함수** (omp와 동일 알고리즘, Rust 이식):
- `find_boundary_echo`, `find_one_sided_boundary_echo`, `find_duplicate_suffix/prefix`, `find_dropped_suffix_closers`
- `compute_delimiter_balance` — `()`/`[]`/`{}` 카운트 (문자열 `'`/`"`/`` ` ```, 라인 주석 `//`, 블록 주석 `/* */` 스킵)
- `read_jsx_payload_tags`, `parse_jsx_payload_tag` — JSX 균형

> **이식 전략**: omp의 5패턴은 **속성 기반 테스트**로 보호된다. Rust에서 `proptest` 크레이트로 동일 불변량(boundary echo 감지 시 교정 후 결과 = 의도한 결과) 검증. omp 회귀 케이스를 그대로 Rust 테스트로 이식.

### 3.7 recovery.rs — 2단계 설계 (session chain MVP → 3-way merge 후순위)

> **리뷰에서 확인된 핵심 리스크**: omp의 recovery 1단계(3-way merge)는 jsdiff
> `Diff.applyPatch(fuzzFactor=0)`에 의존한다. `similar` 크레이트에는 이에 대응하는
> **patch-apply 기능이 없다** — `TextDiff`로 diff는 생성할 수 있으나, 그 diff를 타겟
> 텍스트에 적용(컨텍스트 라인 매칭 + 오프셋 추적 + fuzz 0 불일치 거부)하는 것은
> **100-200 LOC의 정밀 구현**이 별도로 필요하다.
>
> **결정**: recovery를 2단계로 분할한다. M1은 **session chain replay만** 구현하고,
> **3-way merge는 post-M1(M1.5)**로 연기한다. session chain이 cover하는 케이스
> (같은 세션 내 연속 edit로 인한 tag 드리프트)가 실사용의 대다수이며, 외부 수정으로
> 인한 드리프트(3-way merge가 필요한 케이스)는 드물다 — 이 경우 M1은 깔끔하게
> "re-read and retry"로 폴백한다(str_replace 현재 동작과 동등).

#### Phase 1 — session chain replay (M1, 외부 의존 없음)

세션 내 이전 edit가 tag를 진행시킨 경우 — 즉 snapshot이 head가 아닌 경우 —
line count 동일 + anchor 행 내용 동일하면 live에 직접 edit를 replay 한다.
이 경로는 **순수 라인 연산**이므로 외부 diff 크레이트가 필요 없다.

```rust
//! M1 recovery — session chain replay only. 외부 의존 없음.

pub struct Recovery<'a> { pub store: &'a dyn SnapshotStore }

impl<'a> Recovery<'a> {
    pub fn try_recover(&self, args: RecoveryArgs) -> Result<RecoveryResult, RecoveryFailure> {
        let snapshot = self.store.by_hash(&args.path, &args.file_hash)
            .ok_or(RecoveryFailure::NoSnapshot)?;
        let is_head = self.store.head(&args.path).as_ref() == Some(&snapshot);

        if is_head {
            // tag가 head인데 live와 다르다 → 외부 수정. M1은 거부.
            // (post-M1: 3-way merge로 복구 시도)
            return Err(RecoveryFailure::ExternalModification { snapshot });
        }

        // session chain: snapshot.tag → head 사이에 in-session edit가 있었다.
        // line count 동일 + anchor 행 내용 동일 → live에 직접 replay.
        self.replay_session_chain(&snapshot, &args.current_text, &args.edits)
            .ok_or(RecoveryFailure::ChainMismatch)
    }
}

#[derive(Debug)]
pub enum RecoveryFailure {
    NoSnapshot,
    ExternalModification { snapshot: Snapshot },
    ChainMismatch,
}
// → MismatchError로 변환: NoSnapshot/ExternalModification/ChainMismatch 모두
//   "re-read those exact lines" 메시지로 폴백 (M1).

fn replay_session_chain(
    snapshot: &Snapshot,
    live: &str,
    edits: &[Edit],
) -> Option<RecoveryResult> {
    let snap_lines: Vec<&str> = snapshot.text.split('\n').collect();
    let live_lines: Vec<&str> = live.split('\n').collect();

    // 가드 1: line count 동일 (삽입/삭제가 없었다면 행 수 불변)
    if snap_lines.len() != live_lines.len() { return None; }

    // 가드 2: 모든 edit anchor 행의 내용이 snapshot == live
    //   (anchor가 변경되지 않았다면 위치 불변 → 안전하게 live에 적용)
    for edit in edits {
        let anchor_line = edit.anchor_line(); // 1-indexed
        let idx = (anchor_line as usize).saturating_sub(1);
        if idx >= snap_lines.len() { return None; }
        if snap_lines[idx] != live_lines[idx] { return None; }
    }

    // 가드 통과 → live에 edit를 직접 적용.
    let result = apply_edits(live, edits).ok()?;
    if result.text == live { return None; } // no-op
    Some(RecoveryResult {
        text: result.text,
        first_changed_line: result.first_changed_line,
        warnings: vec![RECOVERY_SESSION_CHAIN_WARNING.into()],
    })
}
```

#### Phase 2 — 3-way merge (post-M1 / M1.5, `similar` 기반)

외부 수정(다른 프로세스/도구가 파일을 변경)으로 인한 드리프트를 복구한다.
omp의 `apply_edits_to_snapshot` 경로에 해당.

```rust
//! post-M1 recovery — 3-way merge. similar 크레이트 기반.
//! M1.5에서 구현. M1에서는 RecoveryFailure::ExternalModification → 거부.

fn apply_edits_to_snapshot(
    prev: &str,     // snapshot (tag 시점)
    curr: &str,     // live (현재)
    edits: &[Edit],
    warning: &str,
) -> Option<RecoveryResult> {
    let applied = apply_edits(prev, edits).ok()?;              // tag 버전에 적용
    if applied.text == prev { return None; }

    // prev → applied.text 의 변경분을 curr에 적용 (3-way merge).
    // 단계:
    //   1. similar::TextDiff::from_lines(prev, &applied.text)로 변경 hunk 추출
    //   2. 각 hunk를 curr에서 정확 매칭 (fuzz 0 = 컨텍스트 라인 완전 일치)
    //   3. 매칭 실패 시 None 반환 (거부 → re-read)
    //
    // 주의: similar에는 applyPatch 대응이 없으므로 hunk 매칭 + 적용을
    // 직접 구현한다. 컨텍스트 라인 기반 정렬 + 오프셋 추적 + 불일치 즉시 거부.
    // 예상 규모: 100-200 LOC (별도 모듈 `patch_apply.rs` 권장).
    let merged = patch_apply::apply_changes(curr, prev, &applied.text)?;
    if merged == curr { return None; }
    Some(RecoveryResult {
        text: merged,
        first_changed_line: find_first_changed(curr, &merged),
        warnings: vec![warning.into()],
    })
}
```

> **post-M1 구현 방침** (`patch_apply.rs`):
> - `similar::TextDiff::from_lines(base, modified)`로 `iter_changes()` 순회
> - 각 변경 블록을 (before_context, deleted, inserted, after_context)로 그룹화
> - 타겟 텍스트에서 before/after 컨텍스트로 정확 위치 식별 (fuzz 0)
> - 오프셋이 밀려있으면 추적하여 조정, 컨텍스트 불일치 시 `None` 반환
> - 대안: [`dmp`](https://crates.io/crates/dmp) (google-diff-match-patch 포팅) 검토 —
>   apply 기능 내장. 단, 알고리즘이 다르므로 omp 동작과 정확히 일치하지 않을 수 있음.
> - M1.5 착수 전 **프로토타입으로 검증**: omp 테스트 케이스 5-10개에 대해
>   `similar` 수동 apply vs `dmp` 결과 비교, omp `applyPatch(fuzz=0)` 출력과 대조.

### 3.8 patcher.rs — FS 추상화 + 2단계 커밋

```rust
//! omp patcher.ts 이식. FS는 trait로 추상 (omp Filesystem).

#[async_trait]
pub trait HashlineFs: Send + Sync {
    async fn read_text(&self, path: &str) -> Result<String, HashlineError>;
    async fn write_text(&self, path: &str, text: &str) -> Result<String, HashlineError>;
    async fn preflight_write(&self, path: &str) -> Result<(), HashlineError>;
    fn canonical_path(&self, path: &str) -> String;
    fn is_not_found(&self, err: &HashlineError) -> bool;
}

pub struct Patcher {
    fs: Arc<dyn HashlineFs>,
    snapshots: Arc<dyn SnapshotStore>,
    recovery: RecoveryOwned,
    block_resolver: Option<BlockResolver>,   // [block-ops]
}

impl Patcher {
    pub async fn apply(&self, patch: &Patch) -> Result<PatcherApplyResult, HashlineError> { ... }
    pub async fn preflight(&self, patch: &Patch) -> Result<(), HashlineError> { ... }
    pub async fn prepare(&self, section: &PatchSection) -> Result<PreparedSection, HashlineError> { ... }
    pub async fn commit(&self, prepared: PreparedSection) -> Result<PatchSectionResult, HashlineError> { ... }
}
```

> **FS trait 이유**: oxi-hashline 코어는 tokio/fs에 직결하지 않음 — 테스트 시 mock FS, oxi-cli는 `TokioHashlineFs` 구현체 주입. omp의 `Filesystem` interface와 동일 패턴. **PathGuard 보안 검사**는 `TokioHashlineFs` 내부에서 수행 (oxi-cli 레이어).

### 3.9 parser.rs / tokenizer.rs

omp `parser.ts` + `input.ts` + `tokenizer.ts` 이식. 핵심:
- `split_patch_input(text, opts) -> Patch` — `*** Begin Patch`/`*** End Patch` 인벨로프 분할, `[PATH#HASH]` 헤더별 섹션.
- `PatchSection::parse(&self) -> (Vec<Edit>, Vec<String> warnings)` — 지연 파싱.
- `strip_apply_patch_path_noise` — `Update File:`, `***` 등 모델이 붙이는 노이즈 제거 (omp 정규식 그대로).
- `unquote_hashline_path` — `"path"`/`'path'` 따옴표 제거.

> **파서 전략**: omp는 `Tokenizer` (Lark 문법 기반) 단일 인스턴스 재사용. Rust에서는 `nom` 또는 수동 라인 파서. 의존 최소화 위해 **수동 파서 권장** — Hashline 문법은 단순(키워드 + 숫자 + `+` body). `nom`은 과할 수 있음. M1에서 수동으로 시작, 복잡도 증 시 `nom` 재평가.

### 3.10 HashlineError (thiserror)

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HashlineError {
    #[error("Parse error at line {line}: {msg}")]
    Parse { line: u32, msg: String },
    #[error("File not found: {path}. Use the write tool to create new files.")]
    NotFound { path: String },
    #[error("{0}")]                              // omp missingSnapshotTagMessage
    MissingSnapshotTag(String),
    #[error("{0}")]                              // omp unseenLinesMessage
    UnseenLines(String),
    #[error("{detail}")]                         // MismatchError
    Mismatch { detail: String, expected: String, actual: String },
    #[error("Multiple sections resolve to {path}")]
    DuplicateCanonicalPath { path: String },
    #[error("Edits to {path} resulted in no changes")]
    NoOp { path: String },
    #[error("Line {line} does not exist (file has {total} lines)")]
    LineOutOfBounds { line: u32, total: usize },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[cfg(feature = "block-ops")]
    #[error("Block resolver unavailable for {path}")]
    BlockResolverUnavailable { path: String },
}
```

라이브러리 crate이므로 `thiserror` (AGENTS.md 관례).

---

## 4. oxi-agent 통합 — edit.rs / read.rs 변경

### 4.1 read.rs — snapshot tag 발행

**현재** (`read.rs:227`): `output.push_str(&format!("{:>6}\t{}", line_num, line));`

**변경 후**:
```rust
// 1. 파일 전체 읽기 완료 시 snapshot 기록 + 헤더 발행
let normalized = normalize_to_lf(strip_bom(&raw_content));
let snapshots: Arc<dyn SnapshotStore> = ctx.snapshot_store();   // ToolContext 확장 (§4.3)
let canonical = canonicalize(path);
let hash = snapshots.record(&canonical, &normalized, Some(&displayed_lines));
// 헤더: [path#TAG]
output = format!("[{}{}{}]\n", display_path, '#', hash) + &output;
// 각 행: LINE:TEXT  (기존 {:>6}\t 유지 — 호환성. 또는omp처럼 LINE:TEXT로 전환, §6 결정)
```

**seenLines 기록**: `read`가 offset/limit으로 부분 읽기 시, 표시한 행 번호만 `seen_lines`에 전달. omp와 동일 — `record(path, text, Some(&[1,2,3,...]))`.

> **핵심**: read는 tag를 **발행만**. edit가 tag를 **소비**. 둘은 `SnapshotStore`를 공유. ToolContext에 store 주입 (§4.3).

### 4.2 edit.rs — hashline 모드 추가 (str_replace 보존)

**새 디스패치**:
```rust
async fn apply_edits(root_dir: &Path, input: &EditInput, snapshots: &Arc<dyn SnapshotStore>) -> Result<EditOutput, ToolError> {
    match &input.mode {
        EditMode::StrReplace => Self::apply_str_replace(root_dir, input).await,    // 기존 경로 100%
        EditMode::Hashline { patch_text } => Self::apply_hashline(root_dir, patch_text, snapshots).await,
    }
}

async fn apply_hashline(root: &Path, patch_text: &str, snapshots: &Arc<dyn SnapshotStore>) -> Result<EditOutput, ToolError> {
    let patch = oxi_hashline::split_patch_input(patch_text, Some(root))
        .map_err(|e| e.to_string())?;
    let fs = Arc::new(TokioHashlineFs::new(root));     // PathGuard 내장
    let patcher = oxi_hashline::Patcher::new(fs, snapshots.clone(), None /* block_resolver */);
    let result = patcher.apply(&patch).await.map_err(|e| e.to_string())?;
    // 결과를 EditOutput(diff, first_changed_line, message)로 변환
    Self::format_hashline_result(result)
}
```

**입력 스키마 확장** (`parameters_schema`):
```json
{
  "path": {"type": "string"},
  "edits": {"type": "array", "items": {...}},          // str_replace (기존)
  "old_text": {"type": "string"},                       // legacy (기존)
  "new_text": {"type": "string"},                       // legacy (기존)
  "patch": {"type": "string", "description": "Hashline patch text (*** Begin Patch ... *** End Patch). Mutually exclusive with edits/old_text."},
  "dry_run": {"type": "boolean"},
  "expected_hash": {"type": "string"}                   // str_replace 전용 (hashline은 tag가 있음)
}
```

**`prepare_arguments` 분기**: `patch` 필드 존재 시 `EditMode::Hashline`, 아니면 기존 `EditMode::StrReplace`.

> **불변량**: `patch` 없으면 기존 동작 100%. regression 테스트로 보장 (기존 edit.rs 테스트 전부 통과 필수).

### 4.3 ToolContext 확장 — SnapshotStore 주입

```rust
// oxi-agent/src/tools.rs
pub struct ToolContext {
    pub workspace_dir: PathBuf,
    pub root_dir: Option<PathBuf>,
    pub session_id: Option<String>,
    pub snapshot_store: Option<Arc<dyn oxi_hashline::SnapshotStore>>,   // 신규 (Option)
}
```

> **oxi-agent → oxi-hashline 의존 추가**. `oxi-agent/Cargo.toml`에 `oxi-hashline = { path = "../oxi-hashline" }`.

**주입 경로**:
- `AgentConfig`에 `snapshot_store` 필드 추가.
- `bootstrap.rs`에서 `Arc::new(InMemorySnapshotStore::new())` 생성, `Agent` → `ToolContext`로 스레딩.
- 세션별 1개 store (omp와 동일 — 세션 내 read/edit 체인).

### 4.4 TokioHashlineFs (oxi-cli 또는 oxi-agent)

```rust
pub struct TokioHashlineFs { root: PathBuf }

#[async_trait]
impl HashlineFs for TokioHashlineFs {
    async fn read_text(&self, path: &str) -> Result<String, HashlineError> {
        let guard = PathGuard::new(&self.root);
        let validated = guard.validate_traversal(Path::new(path)).map_err(|e| HashlineError::Io(...))?;
        tokio::fs::read_to_string(&validated).await.map_err(|e| ...)
    }
    async fn write_text(&self, path: &str, text: &str) -> Result<String, HashlineError> {
        // file_mutation_queue를 통해 직렬화 (기존 인프라 재사용)
        let validated = ...;
        global_mutation_queue().with_queue(&validated, || async {
            tokio::fs::write(&validated, text).await
        }).await?;
        Ok(text.to_string())
    }
    // ...
}
```

> **file_mutation_queue 재사용**: omp는 자체 직렬화, oxi는 이미 `global_mutation_queue()`가 per-file 직렬화 제공. `TokioHashlineFs::write_text`가 이를 감싸면 omp와 동등 + 기존 인프라 활용.

---

## 5. str_replace 공존 & 마이그레이션 전략

### 5.1 설정 기반 선택

```rust
// oxi-cli/src/store/settings.rs
pub enum EditFormat {
    StrReplace,       // 기본 (현재 동작)
    Hashline,         // 새 — 시스템 프롬프트가 hashline 문법 가이드
    Auto,             // 모델/작업에 따라 (후순위)
}
pub struct Settings {
    pub edit_format: EditFormat,   // 기본 StrReplace
    ...
}
```

**시스템 프롬프트 빌더** (`build_system_prompt`): `edit_format == Hashline`이면 `oxi-hashline/prompt.md`를 edit 도구 설명에 병합, str_replace 스키마는 숨김(또는 fallback 명시).

### 5.2 롤아웃 단계

| 단계 | edit_format 기본 | hashline 스키마 노출 | 비고 |
|---|---|:-:|---|
| M1.0 | StrReplace | 아니오 | 크레이트 + 도구 구현만, 기본값不变. 개발자가 설정으로 테스트. |
| M1.1 | StrReplace | **옵션** | `Settings::edit_format = Hashline` 시 노출. 얼리 어답터. |
| M1.2 | StrReplace | 옵션 + 벤치마크 | str_replace vs hashline 토큰/pass 비교 (omp 데이터 재현 확인). |
| M1.3 | (데이터 기반 결정) | — | 벤치마크가 압도적이면 Hashline 기본 전환 검토. |

> **언어 정책 패턴 재사용**: omp는 모델별 튜닝. oxi는 "기본 OFF + 명시적 토글"로 시작 (AGENTS.md TUI 언어 정책 v6과 동일 철학). 합의 없는 자동 전환 금지.

### 5.3 expected_hash 호환성 깨짐

기존 `expected_hash` (DefaultHasher 64-hex)는 hashline의 4-hex tag와 **호환 안 됨**. 마이그레이션:
- str_replace 모드는 기존 `expected_hash` 유지 (기존 동작 보존).
- hashline 모드는 `[PATH#TAG]` 헤더로 검증 (`expected_hash` 무시).
- 세션 중 설정 전환 시: snapshot store가 비어 있으므로 첫 read가 새 tag 발행. 이전 hash는 자연 소멸.

---

## 6. 시스템 프롬프트 갱신

`oxi-hashline/src/prompt.md` (omp prompt.md 번역 + oxi 컨텍스트):
- §"headers" — `[PATH#TAG]`, TAG는 최신 read/search의 것.
- §"ops" — SWAP/DEL/INS.PRE/POST/HEAD/TAIL (block op `SWAP.BLK` 등은 후순위 확장, 본 프롬프트에서 제외).
- §"body-rows" — `+TEXT`만, `-old` 없음.
- §"rules" — RE-GROUND AFTER EVERY EDIT, RANGES ARE TIGHT, BODY IS FINAL CONTENT.
- §"example" / §"anti-patterns" / §"critical" — omp 그대로.

**edit 도구 설명** (hashline 모드 시):
```
edit (hashline mode): Apply a hashline patch. The patch names lines by their
numbers from your latest read/search and lists only the final content (+TEXT).
Anchor every section on the [PATH#TAG] header from that read. Stale tag or
surprise? STOP, re-read. See the hashline format spec.
```

---

## 7. 테스트 전략

### 7.1 omp 테스트 이식 (계약 보존)

omp `packages/hashline/src/__tests__/` (있을 경우) + 인라인 테스트를 Rust `#[cfg(test)]`로 이식. **동일 입력 → 동일 출력**이 명세.

| 영역 | omp 동작 | Rust 테스트 |
|---|---|---|
| `compute_file_hash` | xxHash32 벡터 | byte-identical 단정 |
| parser | 섹션 분할, 노이즈 제거 | 회귀 케이스 |
| `apply_edits` | 라인 op 적용 | 단위 |
| `repair_replacement_boundaries` | 5 패턴 | **속성 테스트** (`proptest`) — 교정 후 결과 = 의도 |
| recovery | 3-way merge + session chain | 드리프트 시나리오 |
| snapshot fusion | 동일 내용 재읽기 | recency + tag 재사용 |
| seenLines | 안 본 줄 편집 거부 | 부분 read 후 edit |

### 7.2 oxi 통합 테스트

- `edit.rs` hashline 모드 e2e (read → edit → read 검증).
- str_replace regression (기존 테스트 전부 통과).
- `file_mutation_queue` 직렬화 (hashline write 경로).
- ToolContext `snapshot_store = None` 시 graceful (hashline 모드 비활성화 에러).

### 7.3 CI 게이트 (AGENTS.md 준수)

```bash
cargo nextest run -p oxi-hashline                    # 크레이트 단위
cargo nextest run -p oxi-agent                       # edit.rs 통합
cargo clippy -p oxi-hashline -- -D warnings
# block-ops는 후순위 — 도입 시: cargo clippy -p oxi-hashline --features block-ops -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

## 8. 구현 순서 (M1 세분)

| 서브태스크 | 산출물 | 의존 |
|:-:|---|---|
| M1.1 | 크레이트 스캐폴드 + `Cargo.toml` + 워크스페이스 등록 | — |
| M1.2 | `format.rs` (`compute_file_hash` + omp 벡터 테스트) | M1.1 |
| M1.3 | `normalize.rs` (edit_diff.rs 함수 이전 + 기존 테스트 이동) | M1.1 |
| M1.4 | `types.rs` + `grammar.rs` (상수/enum) | M1.1 |
| M1.5 | `snapshots.rs` (trait + InMemory, lru) | M1.2 |
| M1.6 | `tokenizer.rs` + `parser.rs` (라인 op) | M1.4 |
| M1.7 | `apply.rs` (`apply_edits` + boundary repair 5패턴) | M1.6 |
| M1.8a | `recovery.rs` Phase 1 — session chain replay (외부 의존 없음, §3.7) | M1.5, M1.7 |
| M1.8b | _(post-M1 / M1.5)_ `recovery.rs` Phase 2 — 3-way merge (`similar` 또는 `dmp`, 프로토타입 선행) | M1.8a 안정화 후 |
| M1.9 | `mismatch.rs` + `messages.rs` + `diff_preview.rs` | M1.7 |
| M1.10 | `patcher.rs` (HashlineFs trait + prepare/commit) | M1.5-M1.9 |
| M1.11 | oxi-agent: ToolContext 확장 + `TokioHashlineFs` | M1.10 |
| M1.12 | edit.rs hashline 모드 + read.rs tag 발행 | M1.11 |
| M1.13 | settings (`EditFormat`) + 시스템 프롬프트 | M1.12 |
| M1.14 | omp 테스트 전수 이식 + 벤치마크 | M1.12 |

> M1.1-M1.10은 **oxi-hashline 크레이트 내부** (기존 코드 영향 없음). M1.11부터 기존 코드 수정 — 이 시점까지 regression 제로.

---

## 9. 위험 & 미결정

| 항목 | 상태 | 논의 |
|---|:-:|---|
| read 행 포맷 (`{:>6}\t` 유지 vs `LINE:TEXT` 전환) | 🟡 미결정 | omp는 `LINE:TEXT`. 전환 시 모든 프롬프트 영향. 제안: M1은 기존 `\t` 유지 + 헤더에만 `[path#TAG]` 추가. `LINE:TEXT`는 M1.2에서 평가. |
| 3-way merge patch-apply 구현 | 🟢 **결정됨** (2단계 분할) | M1은 session chain replay만 구현 (외부 의존 없음, §3.7 Phase 1). 3-way merge는 post-M1(M1.5)로 연기 — `similar` 수동 patch-apply(100-200 LOC) 또는 `dmp` 크레이트 검토. M1.5 착수 전 프로토타입으로 omp `applyPatch(fuzz=0)` 출력과 대조 검증 필수. |
| parser 수동 vs `nom` | 🟢 수동 (M1) | 문법 단순. 복잡도 증 시 재평가. |
| 기본 EditFormat 전환 시점 | 🟡 벤치마크 후 | M1.2 데이터가 압도적일 때만. 합의 필수. |
| snapshot store 세션 경계 | 🟢 세션별 1개 | omp와 동일. 세션 종료 시 drop. |
| 멀티섹션 패치 + 동일 파일 | 🟢 거부 | omp `assertUniqueCanonicalPaths` — 병합 권장 메시지. |
| `expected_hash` (str_replace)를 xxhash로 통일? | 🟡 미결정 | 통일 시 일관성↑ but str_replace 경로 호환 깨짐. 제안: str_replace는 기존 유지, hashline만 4-hex. |

---

## 10. 부록: omp 파일 → oxi 모듈 매핑 (참조표)

| omp 파일 (LOC) | oxi 모듈 | 비고 |
|---|---|---|
| `format.ts` (137) | `format.rs` | 상수 + `compute_file_hash` |
| `grammar.lark` (27) | `grammar.rs` | 토큰 정의 (Rust enum) |
| `types.ts` (169) | `types.rs` | Edit/Anchor/Cursor/ApplyResult |
| `tokenizer.ts` (490) | `tokenizer.rs` | LID/range/헤더 토큰화 |
| `parser.ts` (411) | `parser.rs` | parsePatch/parsePatchStreaming |
| `input.ts` (432) | `parser.rs` (병합) | splitPatchInput, PatchSection |
| `normalize.ts` (38) | `normalize.rs` | BOM/CRLF (edit_diff.rs 이전) |
| `snapshots.ts` (180) | `snapshots.rs` | SnapshotStore + InMemory |
| `apply.ts` (998) | `apply.rs` | apply_edits + boundary repair |
| `recovery.ts` (186) | `recovery.rs` (M1: session chain) + `patch_apply.rs` (M1.5: 3-way merge) | 2단계 분할, §3.7 참조 |
| `block.ts` (168) | `block.rs` [block-ops] | resolve_block_edits |
| `patcher.ts` (450) | `patcher.rs` | Patcher: prepare/commit |
| `mismatch.ts` (118) | `mismatch.rs` | MismatchError + 진단 |
| `messages.ts` (240) | `messages.rs` | 사용자 메시지 |
| `diff-preview.ts` (124) | `diff_preview.rs` | CompactDiffPreview |
| `stream.ts` (132) | `stream.rs` | stream_hash_lines |
| `fs.ts` (167) | (patcher.rs 내 HashlineFs trait) | FS 추상 |
| `prompt.md` (143) | `prompt.md` | 모델용 문법 명세 |

**총 omp: ~4.4K LOC TS → oxi 예상 ~3.5K LOC Rust** (타입/제네릭 간결화, 불필요한 JS 패턴 제거).

---

> **다음**: M1.1 (크레이트 스캐폴드) 착수. 본 설계 합의 후 `oxi-hashline/` 디렉토리 생성 + `Cargo.toml` 워크스페이스 등록부터.

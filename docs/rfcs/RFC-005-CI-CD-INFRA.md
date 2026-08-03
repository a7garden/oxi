# RFC-005: CI/CD & 인프라 파이프라인 — 크로스 컴파일, 릴리즈 자동화, 품질 게이트

**상태**: 초안  
**우선순위**: P2 — 프로덕션 배포의 필수 기반  
**현재 완성도**: ~50%  
**목표**: pi 수준의 CI/CD + Rust 특유의 크로스 컴파일 파이프라인  

---

## 1. 문제 정의

### 기존 CI 현황 (이미 구현됨)

oxicode는 이미 2개의 종합적인 GitHub Actions 워크플로우를 보유하고 있다:

#### ci.yml — 품질 게이트 (8개 잡)
| 잡 | 설명 |
|----|------|
| fmt | `cargo fmt --all -- --check` |
| clippy | `cargo clippy --workspace -- -D warnings` + rust-cache |
| test | `cargo test --workspace` + rust-cache |
| test-doc | `cargo test --workspace --doc` + rust-cache |
| build-release | `cargo build --release --workspace` + rust-cache |
| docs | `cargo doc --workspace --no-deps` (RUSTDOCFLAGS="-D warnings") |
| audit | `cargo audit` (보안 감사 전용 잡) |
| deny | `cargo deny check` (의존성 라이선스/보안 검사) |

- `RUSTFLAGS: "-D warnings"` 전역 설정으로 모든 CI에서 경고를 에러로 처리
- `Swatinem/rust-cache@v2` 적용으로 빌드 캐시 최적화

#### release.yml — 릴리즈 자동화 (5개 타겟 매트릭스)
| 타겟 | OS |
|------|-----|
| x86_64-unknown-linux-gnu | ubuntu-latest |
| aarch64-unknown-linux-gnu | ubuntu-latest (cross-compilation) |
| x86_64-apple-darwin | macos-13 |
| aarch64-apple-darwin | macos-latest |
| x86_64-pc-windows-msvc | windows-latest |

- 태그 기반 (`v*`) 자동 빌드 + GitHub Release 업로드
- `softprops/action-gh-release@v2` 사용
- Unix strip, Windows 7zip 패키징

### CI 비교 (수정됨)

| 기능 | pi | oxicode | 격차 |
|------|----|-----|------|
| CI 워크플로우 | 7개 | 2개 (8+5 잡) | 중간 |
| 품질 게이트 | ✅ | ✅ (fmt/clippy/test/doc/deny) | 없음 |
| 보안 감사 | ✅ (npm-audit) | ✅ (cargo audit + deny) | 없음 |
| 릴리즈 자동화 | ✅ (build-binaries.yml) | ✅ (release.yml, 5 타겟) | 약간 |
| 크로스 컴파일 | ✅ (linux/mac/win) | ⚠️ (5 타겟, musl/aarch64-win 없음) | 중간 |
| 바이너리 서명 | ⚠️ | ❌ | 심각 |
| 자동 업데이트 | ✅ (npm update) | ❌ | 심각 |
| 기여자 관리 | ✅ (approve-contributor) | ❌ | 중간 |
| PR 게이트 | ✅ (pr-gate, issue-gate) | ❌ | 중간 |
| 배포 채널 | ✅ (npm, Homebrew) | ❌ (crates.io만) | 심각 |

**가장 큰 격차**: 자동 업데이트, 바이너리 서명, 배포 채널(Homebrew/scoop), 그리고 musl/aarch64-windows 크로스 컴파일 타겟 추가.

---

## 2. 설계 원칙

1. **기존 CI 유지**: ci.yml의 8개 잡과 release.yml의 5개 타겟 매트릭스는 이미 프로덕션 수준이므로 확장만 수행.
2. **GitHub Actions + cross**: `cross` 크레이트로 musl/aarch64-windows 등 추가 타겟 지원.
3. **바이너리 최적화**: UPX 압축, strip, static linking으로 단일 바이너리 배포.
4. **자동 업데이트**: `self_update` 크레이트로 GitHub Releases에서 업데이트 확인.
5. **배포 채널**: Homebrew tap, crates.io, 직접 다운로드, scoop.

---

## 3. 아키텍처

### 3.1 CI 워크플로우 (기존 확장)

```
.github/workflows/
├── ci.yml              # 기존: fmt, clippy, test, test-doc, build-release, docs, audit, deny
├── release.yml          # 기존: 5 타겟 릴리즈 (확장: musl, aarch64-win 추가)
├── build-binaries.yml   # 크로스 플랫폼 바이너리 빌드 (신규 — 추가 타겟)
├── pr-gate.yml          # PR 품질 게이트 (신규)
└── publish.yml          # crates.io / Homebrew / scoop 배포 (신규)
```

> **참고**: security.yml은 별도로 필요하지 않다. ci.yml에 이미 `audit` 잡(cargo-audit)과 `deny` 잡(cargo-deny)이 포함되어 있어 보안 감사를 매 PR/푸시마다 자동 실행한다. 주간 스케줄 감사가 필요한 경우 ci.yml에 schedule 트리거를 추가하는 것으로 충분하다.

#### ci.yml — 기존 품질 게이트 (변경 없음)

```yaml
# 이미 구현됨:
# - fmt: cargo fmt --all -- --check
# - clippy: cargo clippy --workspace -- -D warnings (rust-cache)
# - test: cargo test --workspace (rust-cache)
# - test-doc: cargo test --workspace --doc (rust-cache)
# - build-release: cargo build --release --workspace (rust-cache)
# - docs: cargo doc --workspace --no-deps (RUSTDOCFLAGS="-D warnings")
# - audit: cargo audit
# - deny: cargo deny check
```

#### release.yml — 기존 릴리즈 (확장)

```yaml
# 기존 5개 타겟에 추가:
# - x86_64-unknown-linux-musl (정적 링크)
# - aarch64-unknown-linux-musl (ARM 정적 링크)
# - aarch64-pc-windows-msvc (ARM Windows)

# 기존 기능:
# - 태그 기반 자동 빌드 (v*)
# - Unix strip, Windows 7zip 패키징
# - softprops/action-gh-release@v2 GitHub Release 업로드
```

#### build-binaries.yml — 추가 크로스 플랫폼 빌드 (신규)

```yaml
name: Build Binaries
on:
  workflow_dispatch:
  schedule:
    - cron: '0 3 * * 1'  # 매주 월요일 nightly 빌드

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          # ── musl (정적 링크) ──
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
          # ── ARM Windows ──
          - target: aarch64-pc-windows-msvc
            os: windows-latest

    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      
      - name: Install cross
        if: matrix.use_cross
        run: cargo install cross --locked
      
      - name: Build
        run: |
          ${{ matrix.use_cross && 'cross' || 'cargo' }} build \
            --release \
            --target ${{ matrix.target }} \
            -p oxicode-cli
      
      - name: Strip binary
        if: runner.os != 'Windows'
        run: |
          strip target/${{ matrix.target }}/release/oxicode${{ matrix.suffix }}
      
      - name: Compress (UPX)
        if: runner.os == 'Linux'
        run: |
          sudo apt-get install -y upx-ucl
          upx --best target/${{ matrix.target }}/release/oxicode
      
      - name: Package
        run: |
          mkdir -p dist
          cp target/${{ matrix.target }}/release/oxicode* dist/
          cp LICENSE README.md dist/
          cd dist && tar czf oxicode-${{ matrix.target }}.tar.gz *
      
      - uses: actions/upload-artifact@v4
        with:
          name: oxicode-${{ matrix.target }}
          path: dist/oxicode-${{ matrix.target }}.tar.gz
```

#### pr-gate.yml — PR 품질 게이트 (신규)

```yaml
name: PR Gate
on:
  pull_request:
    types: [opened, synchronize, reopened]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Check PR size
        run: |
          LINES=$(git diff --stat origin/main...HEAD | tail -1 | awk '{print $4+$6}')
          if [ "$LINES" -gt 2000 ]; then
            echo "::warning::PR is large ($LINES lines changed). Consider splitting."
          fi
      
      - name: Check conventional commits
        run: |
          # PR 타이틀이 conventional commit 형식인지 확인
          TITLE="${{ github.event.pull_request.title }}"
          echo "$TITLE" | grep -qE '^(feat|fix|docs|refactor|perf|test|chore|ci|build)(\(.+\))?: .+' || \
            echo "::warning::PR title should follow conventional commits format"
```

### 3.2 자동 업데이트

```rust
/// oxicode-cli/src/updater.rs — 신규

use self_update::cargo_crate_version;

pub struct Updater {
    current_version: String,
    repo_owner: String,
    repo_name: String,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            current_version: cargo_crate_version!().to_string(),
            repo_owner: "earendil-works".into(),
            repo_name: "oxicode".into(),
        }
    }
    
    /// GitHub Releases에서 최신 버전 확인
    pub async fn check_update(&self) -> Result<Option<UpdateInfo>> {
        let release = self_update::backends::github::ReleaseList::configure()
            .repo_owner(&self.repo_owner)
            .repo_name(&self.repo_name)
            .build()?
            .fetch()
            .await?
            .into_iter()
            .next();
        
        match release {
            Some(release) if release.version != self.current_version => {
                Ok(Some(UpdateInfo {
                    version: release.version,
                    date: release.date,
                    body: release.body,
                    assets: release.assets,
                }))
            }
            _ => Ok(None),
        }
    }
    
    /// 업데이트 실행 (현재 바이너리 교체)
    pub async fn update(&self) -> Result<()> {
        let target = self_update::get_target();
        
        self_update::backends::github::Update::configure()
            .repo_owner(&self.repo_owner)
            .repo_name(&self.repo_name)
            .target(&target)
            .bin_name("oxicode")
            .show_download_progress(true)
            .current_version(cargo_crate_version!())
            .build()?
            .update()
            .await?;
        
        Ok(())
    }
}

pub struct UpdateInfo {
    pub version: String,
    pub date: String,
    pub body: Option<String>,
    pub assets: Vec<String>,
}
```

### 3.3 릴리즈 스크립트

```bash
#!/bin/bash
# scripts/release.sh — 버전 벌프 + 태그 + 푸시

set -euo pipefail

VERSION_TYPE="${1:-patch}"  # patch, minor, major

# 1. 워크스페이스 버전 벌프
cargo install cargo-workspaces --locked
cargo ws version "$VERSION_TYPE" --no-git-commit

# 2. 버전 동기화 확인
NEW_VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
echo "New version: $NEW_VERSION"

# 3. Cargo.lock 갱신
cargo generate-lockfile

# 4. 커밋 + 태그
git add -A
git commit -m "chore: release v$NEW_VERSION"
git tag "v$NEW_VERSION"

# 5. 푸시 (CI 트리거)
git push origin main --tags
```

### 3.4 Cargo.toml 빌드 프로파일

```toml
# Cargo.toml — 릴리즈 최적화

[profile.release]
opt-level = 3          # 최대 최적화
lto = true             # 링크 타임 최적화
codegen-units = 1      # 단일 코드젠 유닛 (더 느리지만 더 작은 바이너리)
panic = "abort"        # unwinder 제거 → 바이너리 크기 감소
strip = true           # 심볼 스트립

[profile.release.package."*"]
opt-level = 3
```

### 3.5 설치 방법

```markdown
## 설치

# Homebrew (macOS/Linux)
brew install earendil-works/tap/oxicode

# Cargo
cargo install oxicode-cli

# 직접 다운로드
curl -fsSL https://github.com/earendil-works/oxicode/releases/latest/download/oxicode-$(uname -m)-$(uname -s).tar.gz | tar xz
sudo mv oxicode /usr/local/bin/

# Windows (scoop)
scoop bucket add oxicode https://github.com/earendil-works/oxicode-scoop
scoop install oxicode

# 자동 업데이트
oxicode update              # 또는 설정에서 auto_update = true
```

### 3.6 Homebrew Formula 자동 생성

```yaml
# .github/workflows/publish.yml
jobs:
  homebrew:
    needs: release
    runs-on: ubuntu-latest
    steps:
      - name: Update Homebrew tap
        run: |
          # GitHub Release에서 다운로드 URL 생성
          # SHA256 체크섬 계산
          # formula.rb 템플릿 렌더링
          # homebrew-tap 리포지토리에 PR
```

---

## 4. 구현 계획

### Phase 1: 릴리즈 확장 (1주)

| 작업 | 산출물 |
|------|--------|
| release.yml 타겟 추가 | musl (x64 + arm64), aarch64-windows |
| cross 설정 | Cross.toml + 시스템 의존성 |
| 바이너리 최적화 | UPX + LTO 검증 |

> **참고**: ci.yml은 이미 완벽하게 갖춰져 있으므로 Phase 1에서 CI 강화 작업은 불필요하다.

### Phase 2: 자동 업데이트 (1주)

| 작업 | 산출물 |
|------|--------|
| updater.rs | self_update 기반 업데이터 |
| /update 명령어 | TUI/RPC에서 업데이트 트리거 |
| 체크섬 검증 | SHA256 검증 |

### Phase 3: PR 게이트 + 배포 채널 (1주)

| 작업 | 산출물 |
|------|--------|
| pr-gate.yml | PR 사이즈, 컨벤션, 라벨 |
| Homebrew tap | 자동 formula 생성 |
| crates.io | cargo publish 파이프라인 |
| install.sh | curl 파이프 설치 스크립트 |

---

## 5. 새 의존성

```toml
[dependencies]
self_update = { version = "0.41", features = ["github"] }  # 자동 업데이트
sha2 = "0.10"                                                # 체크섬
```

---

## 6. 성공 기준

- [x] CI: PR당 fmt + clippy + test + doc + audit + deny 자동 실행 (이미 구현됨)
- [x] 보안 감사: cargo audit + cargo deny 매 PR/푸시마다 자동 실행 (이미 구현됨)
- [x] 릴리즈: 태그 푸시 시 자동 바이너리 빌드 + GitHub Release 업로드 (이미 구현됨, 5 타겟)
- [ ] 크로스 컴파일: 8개 타겟 (musl x64/arm64, aarch64-windows 추가)
- [ ] 자동 업데이트: `oxicode update` 실행 시 최신 버전 설치
- [ ] 배포: Homebrew + crates.io + 직접 다운로드
- [ ] PR 게이트: PR 사이즈, 컨벤션 커밋 검사

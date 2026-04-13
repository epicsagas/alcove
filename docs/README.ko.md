<p align="center">
  <img src="../alcove.png" alt="Alcove" width="100%" />
</p>

<p align="center"><strong>당신의 AI 에이전트는 프로젝트를 모릅니다. Alcove가 해결합니다.</strong></p>

<p align="center">
  <a href="../README.md">English</a> ·
  <a href="README.ko.md">한국어</a> ·
  <a href="README.ja.md">日本語</a> ·
  <a href="README.zh-CN.md">简体中文</a> ·
  <a href="README.es.md">Español</a> ·
  <a href="README.hi.md">हिन्दी</a> ·
  <a href="README.pt-BR.md">Português</a> ·
  <a href="README.de.md">Deutsch</a> ·
  <a href="README.fr.md">Français</a> ·
  <a href="README.ru.md">Русский</a>
</p>

<p align="center">
  <a href="https://glama.ai/mcp/servers/epicsagas/alcove"><img src="https://glama.ai/mcp/servers/epicsagas/alcove/badges/score.svg" height="20" alt="Glama MCP Score" /></a>
  <a href="https://crates.io/crates/alcove"><img src="https://img.shields.io/crates/v/alcove.svg" alt="crates.io" /></a>
  <a href="https://crates.io/crates/alcove"><img src="https://img.shields.io/crates/d/alcove.svg" alt="Downloads" /></a>
  <a href="../LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-blue.svg" alt="License" /></a>
  <a href="https://buymeacoffee.com/epicsaga"><img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-FFDD00?style=flat&logo=buy-me-a-coffee&logoColor=black" alt="Buy Me a Coffee" /></a>
</p>

Alcove는 모든 AI 코딩 에이전트가 프라이빗 프로젝트 문서를 읽고 관리할 수 있게 해줍니다 — 공개 저장소에 유출되지 않으면서.

PRD, 아키텍처 결정, 시크릿 맵, 내부 런북을 한 곳에 보관하세요. 모든 MCP 호환 에이전트가 같은 도구를 얻고, 모든 프로젝트에서 동작하며, 프로젝트별 설정이 필요 없습니다.

## 문제

두 가지 나쁜 선택지가 있습니다.

**선택 A: `CLAUDE.md` / `AGENTS.md`에 문서 넣기**
모든 실행마다 모든 파일이 컨텍스트 창에 주입됩니다.
짧은 규칙에는 통하지만 실제 프로젝트 문서에서는 망가집니다.
아키텍처 파일 10개 = 컨텍스트 팽창 = 느리고, 비싸고, 덜 정확한 응답.

**선택 B: 문서를 넣지 않기**
에이전트가 이미 문서화된 요구사항을 지어냅니다.
이미 내린 결정의 제약을 무시합니다.
매 세션마다 같은 것을 설명하도록 요청합니다.

어느 쪽도 확장되지 않습니다. 5개 프로젝트와 3개 에이전트에 걸쳐 곱하면, 각각 다르게 설정됩니다. 전환할 때마다 컨텍스트를 잃습니다.

## Alcove가 해결하는 방법

Alcove는 모든 프라이빗 문서를 프로젝트별로 정리된 **하나의 공유 저장소**에 보관합니다. MCP 호환 에이전트라면 동일한 방식으로 접근할 수 있습니다 — Claude Code, Cursor, Gemini CLI, Codex 어디서든 상관없습니다.

```
~/projects/my-app $ claude "인증은 어떻게 구현되어 있나요?"

  → Alcove가 프로젝트 감지: my-app
  → ~/documents/my-app/ARCHITECTURE.md 읽기
  → 에이전트가 실제 프로젝트 컨텍스트로 답변
```

```
~/projects/my-api $ codex "API 설계를 검토해줘"

  → Alcove가 프로젝트 감지: my-api
  → 동일한 문서 구조, 동일한 접근 패턴
  → 다른 프로젝트, 같은 워크플로우
```

**에이전트를 언제든 전환하세요. 프로젝트를 언제든 전환하세요. 문서 레이어는 표준화되어 있습니다.**

## 주요 기능

- **하나의 문서 저장소, 여러 프로젝트** — 프라이빗 문서를 프로젝트별로 정리하고 한 곳에서 관리
- **한 번 설정, 모든 에이전트** — 한 번 설정하면 모든 MCP 호환 에이전트가 동일한 접근 권한을 얻음
- **CWD 기반 프로젝트 자동 감지** — 프로젝트별 설정 불필요
- **범위 지정 접근** — 각 프로젝트는 자신의 문서만 볼 수 있음
- **스마트 검색** — BM25 랭킹 검색과 자동 인덱싱; 가장 관련성 높은 문서를 먼저 찾고, 필요 시 grep으로 폴백
- **크로스 프로젝트 검색** — `scope: "global"`로 모든 프로젝트를 한 번에 검색 — 개인 지식 베이스로 활용
- **프라이빗 문서는 프라이빗으로 유지** — 민감한 문서(시크릿 맵, 내부 결정, 기술 부채)가 공개 저장소에 들어가지 않음
- **표준화된 문서 구조** — `policy.toml`로 모든 프로젝트와 팀에 일관된 문서를 적용
- **크로스 레포 감사** — 프로젝트 저장소에 잘못 배치된 내부 문서를 찾아 수정 제안
- **문서 검증** — 누락된 파일, 미작성 템플릿, 필수 섹션 확인
- **시맨틱 린트** — 깨진 위키링크, 고아 파일, 오래된 WIP/DRAFT 마커, 2년 이상 지난 날짜 표현 자동 감지
- **외부 볼트 가져오기** — Obsidian 등의 노트를 한 명령으로 doc-repo에 추가; 파일명·내용 기반 프로젝트 자동 라우팅
- **9개 이상 에이전트 지원** — Claude Code, Cursor, Claude Desktop, Cline, OpenCode, Codex, Copilot, Antigravity, Gemini CLI

## Alcove를 사용하는 이유

| Alcove 없이 | Alcove와 함께 |
|-------------|--------------|
| 내부 문서가 Notion, Google Docs, 로컬 파일에 흩어져 있음 | 하나의 문서 저장소, 프로젝트별로 구조화 |
| 각 AI 에이전트마다 문서 접근을 별도로 설정 | 한 번 설정, 모든 에이전트가 동일한 접근 공유 |
| 프로젝트를 전환하면 문서 컨텍스트를 잃음 | CWD 자동 감지, 즉시 프로젝트 전환 |
| 에이전트 검색이 무작위 매칭 줄을 반환 | BM25 랭킹 검색 — 최적 매치 우선, 자동 인덱싱 |
| "인증 관련 노트 전부 검색" — 불가능 | 글로벌 검색으로 모든 프로젝트를 한 번에 쿼리 |
| 민감한 문서가 프로젝트 저장소에 섞여 있거나 여기저기 흩어져 있음 | 프라이빗 문서는 프로젝트 저장소와 물리적으로 분리 |
| 프로젝트와 팀원마다 문서 구조가 다름 | `policy.toml`로 모든 프로젝트에 표준 적용 |
| 문서가 완성되었는지 확인할 방법이 없음 | `validate`가 누락된 파일, 빈 템플릿, 누락된 섹션을 감지 |
| 오래된 링크나 WIP 마커를 놓치기 쉬움 | `lint`가 깨진 링크, 고아 파일, 오래된 마커를 자동 감지 |
| Obsidian 등 외부 노트가 고립된 상태로 남아 있음 | `promote`로 외부 노트를 한 명령으로 doc-repo에 통합 |

## 빠른 시작

```bash
# macOS
brew install epicsagas/alcove/alcove

# Linux / Windows — 사전 빌드 바이너리 (빠름, 컴파일 불필요)
cargo install cargo-binstall
cargo binstall alcove

# 모든 플랫폼 — 소스에서 빌드
cargo install alcove

# 클론 후 빌드
git clone https://github.com/epicsagas/alcove.git
cd alcove
make install

alcove setup
```

**선택적 의존성**

| 도구 | 목적 | 설치 |
|---|---|---|
| `pdftotext` (poppler) | PDF 전문 텍스트 추출 — PDF 검색에 필요 | macOS: `brew install poppler` · Debian/Ubuntu: `apt install poppler-utils` · Fedora: `dnf install poppler-utils` · Windows: [poppler for Windows](https://github.com/oschwartz10612/poppler-windows/releases) |

`pdftotext` 없이도 동작하지만, 일부 PDF는 내장 파서로 추출에 실패할 수 있습니다. `alcove doctor`로 설치 상태를 확인하세요.

이것만 하면 됩니다. `setup`이 대화형으로 모든 것을 안내합니다:

1. 문서가 어디에 있는지
2. 어떤 문서 카테고리를 추적할지
3. 선호하는 다이어그램 형식
4. 어떤 AI 에이전트를 설정할지 (MCP + 스킬 파일)

설정을 변경하려면 언제든 `alcove setup`을 다시 실행하세요. 이전 선택을 기억합니다.

## 작동 방식

```mermaid
flowchart LR
    subgraph Projects["프로젝트"]
        A1["my-app/\n  src/ ..."]
        A2["my-api/\n  src/ ..."]
    end

    subgraph Docs["프라이빗 문서 (하나의 저장소)"]
        D1["my-app/\n  PRD.md\n  ARCH.md"]
        D2["my-api/\n  PRD.md\n  ..."]
        P1["policy.toml"]
    end

    subgraph Agents["모든 MCP 에이전트"]
        AG["Claude Code · Cursor\nGemini CLI · Codex · Copilot\n+4 more"]
    end

    subgraph MCP["Alcove MCP 서버"]
        T["search · get_file\noverview · audit\ninit · validate"]
    end

    A1 -- "CWD 감지" --> D1
    A2 -- "CWD 감지" --> D2
    Agents -- "stdio MCP" --> MCP
    MCP -- "범위 지정 접근" --> Docs
```

문서는 별도 디렉토리(`DOCS_ROOT`)에 프로젝트별 폴더로 정리됩니다. Alcove는 거기서 관리하고 제공합니다 — stdio를 통해 MCP 호환 AI 에이전트에게. 에이전트는 `get_doc_file("PRD.md")` 같은 도구를 호출하여 어떤 에이전트를 사용하든 프로젝트별 답변을 얻습니다.

## 문서 분류

Alcove는 문서를 다음과 같이 분류합니다:

| 분류 | 위치 | 예시 |
|------|------|------|
| **doc-repo-required** | Alcove (프라이빗) | PRD, Architecture, Decisions, Conventions |
| **doc-repo-supplementary** | Alcove (프라이빗) | Deployment, Onboarding, Testing, Runbook |
| **reference** | Alcove `reports/` 폴더 | 감사 보고서, 벤치마크, 분석 |
| **project-repo** | GitHub 저장소 (공개) | README, CHANGELOG, CONTRIBUTING |

`audit` 도구는 doc-repo와 로컬 프로젝트 디렉토리를 양쪽 모두 스캔하고 조치를 제안합니다 — 프라이빗 PRD에서 공개 README를 생성하거나, 잘못 배치된 리포트를 alcove로 가져오는 등.

## MCP 도구

| 도구 | 기능 |
|------|------|
| `get_project_docs_overview` | 분류 및 크기와 함께 모든 문서 목록 표시 |
| `search_project_docs` | 스마트 검색 — BM25 랭킹 또는 grep 자동 선택, `scope: "global"`로 크로스 프로젝트 검색 지원 |
| `get_doc_file` | 경로로 특정 문서 읽기 (대용량 파일은 `offset`/`limit` 지원) |
| `list_projects` | 문서 저장소의 모든 프로젝트 표시 |
| `audit_project` | 크로스 레포 감사 — doc-repo와 로컬 프로젝트 디렉토리를 스캔하고 조치 제안 |
| `init_project` | 새 프로젝트 문서 스캐폴딩 (내부+외부 문서, 선택적 파일 생성) |
| `validate_docs` | 팀 정책(`policy.toml`)에 따라 문서 검증 |
| `rebuild_index` | 전문 검색 인덱스 재빌드 (보통 자동) |
| `check_doc_changes` | 마지막 인덱스 빌드 이후 추가·수정·삭제된 문서 감지 |
| `lint_project` | 시맨틱 린트 — 깨진 링크, 고아 파일, 오래된 마커, 오래된 날짜 표현 |
| `promote_document` | 외부 볼트의 파일을 alcove doc-repo에 복사 또는 이동 |

## CLI

```
alcove              MCP 서버 시작 (에이전트가 호출)
alcove setup        대화형 설정 — 언제든 다시 실행하여 재설정
alcove doctor       설치 상태 진단
alcove validate     정책에 따라 문서 검증 (--format json, --exit-code)
alcove lint         시맨틱 린트 — 깨진 링크, 고아 파일, 오래된 마커 (--format json)
alcove promote      외부 볼트의 노트를 doc-repo에 가져오기
alcove index        검색 인덱스 업데이트 (증분 — 변경된 파일만)
alcove rebuild      검색 인덱스 전체 재구축 (스키마 변경 후 사용)
alcove search       터미널에서 문서 검색
alcove serve        HTTP RAG 서버 시작 (외부 접근용)
alcove enable       macOS 로그인 항목으로 등록 및 백그라운드 서버 시작
alcove disable      로그인 항목 해제 및 서버 중지
alcove start        백그라운드 서버 시작
alcove stop         백그라운드 서버 중지
alcove restart      백그라운드 서버 재시작
alcove uninstall    스킬, 설정 및 레거시 파일 제거
```

### 린트

```bash
# 현재 프로젝트 린트 (CWD에서 자동 감지)
alcove lint

# 특정 프로젝트 지정
alcove lint --project my-app

# CI용 머신 리더블 출력
alcove lint --format json
```

린트는 네 가지를 검사합니다:

| 검사 항목 | 감지 내용 |
|-----------|-----------|
| `broken-link` | 존재하지 않는 파일을 가리키는 `[[위키링크]]` 또는 `[텍스트](경로)` |
| `orphan` | 다른 문서에서 링크되지 않는 파일 |
| `stale-marker` | WIP / TODO / FIXME / DRAFT / DEPRECATED 마커 |
| `stale-date` | 2년 이상 지난 날짜 표현 (예: "as of 2022") |

### 프로모트

```bash
# Obsidian 노트를 doc-repo에 복사 (자동으로 프로젝트 라우팅)
alcove promote ~/my-brain/Projects/auth-notes.md

# 특정 프로젝트 지정
alcove promote ~/my-brain/Projects/auth-notes.md --project my-app

# 복사 대신 이동
alcove promote ~/my-brain/Projects/auth-notes.md --mv
```

일치하는 프로젝트가 없는 파일은 수동 검토를 위해 `inbox/`에 저장됩니다.

### 백그라운드 서버

Alcove를 REST API로 접근 가능한 상주 HTTP RAG 서버로 실행할 수 있습니다. 외부 통합, 대시보드, 또는 비-MCP 클라이언트에 유용합니다.

```bash
# 포그라운드에서 서버 시작
alcove serve                       # 기본: 127.0.0.1:8080
alcove serve --port 9090           # 포트 지정
alcove serve --token my-secret     # Bearer 토큰 인증
alcove serve --host 0.0.0.0       # 전체 인터페이스에서 수신
```

#### macOS 로그인 항목 (launchd)

Alcove를 macOS 로그인 항목으로 등록하면 로그인 시 자동으로 HTTP 서버가 시작되고 백그라운드에서 계속 실행됩니다:

```bash
# 등록 및 시작 (재부팅 후에도 유지)
alcove enable

# 라이프사이클 관리
alcove stop         # 서버 중지
alcove start        # 다시 시작
alcove restart      # 중지 후 시작

# 등록 해제 (서버 중지 + 로그인 항목 제거)
alcove disable
```

LaunchAgent가 `~/Library/LaunchAgents/com.epicsagas.alcove.plist`에 설치됩니다. 로그는 `~/.alcove/logs/`에 기록됩니다.

> **참고:** MCP 서버(stdio)는 Claude Code 세션 시작 시 자동으로 시작됩니다 — `enable`은 필요 없습니다. `enable`은 외부 접근을 위한 HTTP RAG 서버를 상시 실행하려는 경우에만 사용하세요.

## 검색

Alcove는 자동으로 최적의 검색 전략을 선택합니다. 검색 인덱스가 존재하면 **BM25 랭킹 검색** ([tantivy](https://github.com/quickwit-oss/tantivy) 기반)을 사용하여 관련도 점수로 정렬된 결과를 반환합니다. 인덱스가 없으면 grep으로 폴백합니다. 사용자가 신경 쓸 필요 없습니다.

### 하이브리드 검색 (RAG)

Alcove는 BM25와 **벡터 유사도 검색** ([fastembed](https://github.com/ankane/fastembed-rs) 기반)을 결합한 **하이브리드 검색**을 지원합니다.

`alcove setup` 과정에서 임베딩 모델을 선택하고 즉시 다운로드할 수 있습니다. 모델을 수동으로 관리할 수도 있습니다:

```bash
# 임베딩 모델 설정 및 다운로드
alcove model set MultilingualE5Small
alcove model download

# 모델 상태 확인
alcove model status
```

#### 모델 선택

| 모델 | 디스크 | 차원 | 언어 지원 | 추천 용도 |
|------|--------|------|-----------|-----------|
| `SnowflakeArcticEmbedXSQ` | 15 MB | 384 | 영어 | CI, 리소스 제약 환경 |
| `SnowflakeArcticEmbedXS` | 30 MB | 384 | 영어 | 빠른 영어 전용 인덱싱 |
| `SnowflakeArcticEmbedSQ` | 65 MB | 384 | 영어 | 품질·용량 균형 (영어) |
| `SnowflakeArcticEmbedS` | 130 MB | 384 | 영어 | 영어 검색 품질 |
| **`MultilingualE5Small`** | **235 MB** | **384** | **100+ 언어** | **기본값 — 다국어·혼합 언어 프로젝트** |
| `SnowflakeArcticEmbedMQ` | 200 MB | 768 | 영어 | 고품질, 양자화 |
| `SnowflakeArcticEmbedM` | 400 MB | 768 | 영어 | 최고 영어 품질 |
| `MultilingualE5Base` | 555 MB | 768 | 100+ 언어 | 다국어 품질 향상 |
| `MultilingualE5Large` | 2.2 GB | 1024 | 100+ 언어 | 최고 다국어 품질 |
| `BGEM3` | 2.3 GB | 1024 | 100+ 언어 | 최첨단 다국어 |

**Q (양자화) 변형**은 int8 양자화를 사용합니다 — 디스크 ~50% 절감, 약간의 recall 저하, 일반 문서 검색에서는 실질적인 정확도 손실 없음. 메모리가 제한적인 환경에서는 XSQ/SQ/MQ 변형을 사용하세요.

모델이 준비되면 Alcove는 CLI 검색과 에이전트 기반 MCP 도구 모두에서 자동으로 하이브리드 검색을 사용합니다. 이는 다국어 프로젝트나 복잡한 의미론적 쿼리에 특히 효과적입니다.

```bash
# 현재 프로젝트 검색 (CWD에서 자동 감지)
alcove search "authentication flow"

# 정확한 부분 문자열 매칭이 필요하면 grep 모드 강제
alcove search "FR-023" --mode grep
```

인덱스는 MCP 서버 시작 시 백그라운드에서 자동으로 빌드되며, 파일 변경을 감지하면 자동으로 재빌드합니다. 크론 잡도, 수동 작업도 필요 없습니다.

**에이전트 사용법:** 에이전트는 쿼리로 `search_project_docs`를 호출하기만 하면 됩니다. Alcove가 랭킹, 중복 제거(파일당 하나의 결과), 크로스 프로젝트 검색, 폴백을 모두 처리합니다. 에이전트가 검색 모드를 선택할 필요가 없습니다.

#### 인덱스 생명주기

`alcove index`와 `alcove rebuild`의 차이:

| 명령어 | 동작 | 사용 시점 |
|--------|------|-----------|
| `alcove index` | 증분 업데이트 — 새 파일·변경 파일만 처리 | 기본: 문서 추가·수정 후 |
| `alcove rebuild` | 전체 재구축 — 모든 인덱스 데이터 삭제 후 재생성 | 모델 변경 후, 인덱스 손상 시 |

**최초 설정:**

```bash
# 1단계: 설정 직후 BM25 검색 즉시 사용 가능
alcove index            # 전문 검색 인덱스 빌드 (모델 불필요)

# 2단계: 하이브리드 검색 활성화 (선택 사항이지만 권장)
alcove model set MultilingualE5Small
alcove model download   # ~235 MB 다운로드

# 3단계: 기존 문서 전체 벡터 인덱스 빌드
alcove rebuild          # 최초 1회 전체 재빌드
                        # ⚠ 피크 RAM = 모델 크기 + corpus 벡터 (아래 참고)

# 이후: 증분 업데이트만으로 충분
alcove index            # 빠름 — 변경 파일만 재임베딩
```

**모델 변경:**

```bash
alcove model set SnowflakeArcticEmbedS   # 모델 변경
alcove rebuild                            # 필수: 벡터는 모델별로 호환 불가
```

**rebuild 시 메모리:**  
피크 RAM = 모델 크기 + HNSW 그래프 구성을 위해 RAM에 올라가는 모든 문서 벡터. `MultilingualE5Small` 기준 ~3,500개 문서 기준 피크 약 700 MB. 구조적으로 불가피하며, rebuild 완료 후 `[memory]` 설정에 따라 평상시 50–200 MB로 감소합니다.

## 프로젝트 감지

기본적으로 Alcove는 터미널의 작업 디렉토리(CWD)에서 현재 프로젝트를 감지합니다. `MCP_PROJECT_NAME` 환경 변수로 오버라이드할 수 있습니다:

```bash
MCP_PROJECT_NAME=my-api alcove
```

CWD가 문서 저장소의 프로젝트 이름과 일치하지 않을 때 유용합니다.

## 문서 정책

문서 저장소의 `policy.toml`로 팀 전체 문서 표준을 정의합니다:

```toml
[policy]
enforce = "strict"    # strict | warn

[[policy.required]]
name = "PRD.md"
aliases = ["prd.md", "product-requirements.md"]

[[policy.required]]
name = "ARCHITECTURE.md"

  [[policy.required.sections]]
  heading = "## Overview"
  required = true

  [[policy.required.sections]]
  heading = "## Components"
  required = true
  min_items = 2
```

정책 파일은 **프로젝트** (`<project>/.alcove/policy.toml`) > **팀** (`DOCS_ROOT/.alcove/policy.toml`) > **내장 기본값** (config.toml의 core 파일 목록) 우선순위로 적용됩니다. 이를 통해 모든 프로젝트에 일관된 문서 품질을 보장하면서 프로젝트별 오버라이드를 허용합니다.

## 설정

설정 파일 위치: `~/.config/alcove/config.toml`:

```toml
docs_root = "/Users/you/documents"

[core]
files = ["PRD.md", "ARCHITECTURE.md", "PROGRESS.md", "DECISIONS.md", "CONVENTIONS.md", "SECRETS_MAP.md", "DEBT.md"]

[team]
files = ["ENV_SETUP.md", "ONBOARDING.md", "DEPLOYMENT.md", "TESTING.md", ...]

[public]
files = ["README.md", "CHANGELOG.md", "CONTRIBUTING.md", "SECURITY.md", ...]

[diagram]
format = "mermaid"

[memory]
reader_ttl_secs   = 300   # N초 후 유휴 IndexReader 해제 (0 = 해제 안 함)
max_cached_readers = 1    # RAM에 동시에 유지할 최대 IndexReader 수
model_unload_secs  = 600  # N초 동안 임베딩 미사용 시 모델 언로드 (0 = 언로드 안 함)
max_hnsw_cache     = 3    # 동시에 메모리에 유지할 최대 HNSW 그래프 수
```

모든 설정은 `alcove setup`으로 대화형으로 진행됩니다. 파일을 직접 편집할 수도 있습니다.

**메모리 사용 참고:** 최초 인덱싱 또는 전체 rebuild 시, Alcove는 임베딩 모델(~235–500MB)을 로드하고 HNSW 그래프 구성을 위해 모든 문서 벡터를 RAM에 올립니다. 이 시점의 피크 메모리는 corpus 크기에 비례하며 구조적으로 불가피합니다. 위 `[memory]` 설정은 인덱싱 완료 후 *평상시* 메모리를 제어합니다.

## 지원 에이전트

| 에이전트 | MCP | 스킬 |
|----------|-----|------|
| Claude Code | `~/.claude.json` | `~/.claude/skills/alcove/` |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/skills/alcove/` |
| Claude Desktop | 플랫폼 설정 | — |
| Cline (VS Code) | VS Code globalStorage | `~/.cline/skills/alcove/` |
| OpenCode | `~/.config/opencode/opencode.json` | `~/.opencode/skills/alcove/` |
| Codex CLI | `~/.codex/config.toml` | `~/.codex/skills/alcove/` |
| Copilot CLI | `~/.copilot/mcp-config.json` | `~/.copilot/skills/alcove/` |
| Antigravity | `~/.gemini/antigravity/mcp_config.json` | — |
| Gemini CLI | `~/.gemini/settings.json` | `~/.gemini/skills/alcove/` |

## 지원 언어

CLI는 시스템 로케일을 자동 감지합니다. `ALCOVE_LANG` 환경 변수로 오버라이드할 수도 있습니다.

| 언어 | 코드 |
|------|------|
| English | `en` |
| 한국어 | `ko` |
| 简体中文 | `zh-CN` |
| 日本語 | `ja` |
| Español | `es` |
| हिन्दी | `hi` |
| Português (Brasil) | `pt-BR` |
| Deutsch | `de` |
| Français | `fr` |
| Русский | `ru` |

```bash
# 언어 오버라이드
ALCOVE_LANG=ko alcove setup
```

## 업데이트

```bash
# Homebrew
brew upgrade epicsagas/alcove/alcove

# cargo-binstall
cargo binstall alcove

# 소스에서
cargo install alcove
```

## 삭제

```bash
alcove uninstall          # 스킬 & 설정 제거
cargo uninstall alcove    # 바이너리 제거
```

## 에코시스템

### [obsidian-forge](https://github.com/epicsagas/obsidian-forge)

Alcove는 **obsidian-forge**와 자연스럽게 연동됩니다. obsidian-forge는 Obsidian 볼트 생성기이자 자동화 데몬입니다. obsidian-forge로 지식 그래프를 구축한 뒤, `alcove promote`로 노트를 doc-repo에 가져오세요 — AI 에이전트가 컨텍스트 낭비 없이 랭킹 검색으로 프로젝트 지식 베이스를 활용할 수 있습니다.

```
obsidian-forge (개인 지식)   →   alcove promote   →   alcove (프로젝트 문서)
  볼트 / 인박스 / 그래프          한 명령                BM25 + 벡터 검색
```

## 기여

버그 리포트, 기능 요청, 풀 리퀘스트를 환영합니다. 논의를 시작하려면 [GitHub](https://github.com/epicsagas/alcove/issues)에 이슈를 열어주세요.

## 라이선스

Apache-2.0

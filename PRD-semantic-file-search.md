# PRD: Semantic File Search Engine

## 1. Overview

### 1.1 Product Name
**semantic-file-search**

### 1.2 Purpose
A desktop application that searches local file system documents using natural language.
Users can find files with queries like "Where's the Q3 revenue report?" or "Find the contract file from Mr. Kim."

### 1.3 Core Value Proposition
- **Content-based** semantic search, not filename matching
- Supports diverse formats: PDF, DOCX, PPTX, XLSX, TXT, images
- Handles scanned PDFs (image-based) natively via Gemini vision
- **완전한 단일 PC 실행** — 인덱스 DB, 설정, 임베딩 캐시 등 모든 데이터가 사용자 로컬 머신에 저장. 외부 네트워크 통신은 Gemini API 호출만 발생
- **서버/클라우드 인프라 불필요** — 별도 서버, DB 서버, 벡터 DB 서비스 등 외부 인프라에 일체 의존하지 않음
- Cross-platform desktop app (macOS, Ubuntu Linux, Windows) via Tauri

---

## 2. Architecture

### 2.1 Single Client Architecture

모든 컴포넌트는 단일 사용자의 로컬 PC에서 실행된다. 외부 의존은 Gemini API 호출(HTTPS)뿐이며, 나머지 모든 처리/저장은 로컬에서 수행한다.

```
┌─────────────────────────────────────────────────────────┐
│  Single Client PC (macOS / Ubuntu / Windows)             │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  Tauri Desktop App (single Rust process)             │ │
│  │                                                      │ │
│  │  ┌─────────────┐  ┌──────────────┐  ┌────────────┐ │ │
│  │  │ File Crawler  │  │ Search Engine │  │ Local Index │ │ │
│  │  │  (notify)    │  │(Vector Search)│  │  (SQLite)  │ │ │
│  │  └──────┬───────┘  └──────┬───────┘  └──────┬─────┘ │ │
│  │         │                 │                  │       │ │
│  │  ┌──────▼───────┐         │                  │       │ │
│  │  │ anytomd       │         │                  │       │ │
│  │  │ (Rust crate)  │         │                  │       │ │
│  │  │ DOCX/PPTX/   │         │                  │       │ │
│  │  │ XLSX/CSV→MD   │         │                  │       │ │
│  │  └──────┬───────┘         │                  │       │ │
│  │         │ Markdown + images│                  │       │ │
│  │         ▼                 │                  │       │ │
│  │  ┌──────────────────────────────────────┐    │       │ │
│  │  │ Gemini API Client (reqwest)           │    │       │ │
│  │  └──────────────────┬───────────────────┘    │       │ │
│  │                     │ summary + embedding     │       │ │
│  │                     └────────────────────────▶│       │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  Local Storage (app_data_dir per OS)                  │ │
│  │  - SQLite DB (index + pending queue)                  │ │
│  │  - config.json                                        │ │
│  │  - logs/                                              │ │
│  └─────────────────────────────────────────────────────┘ │
└──────────────────────────┬──────────────────────────────┘
                           │ HTTPS (Gemini API only — 유일한 외부 통신)
                           ▼
                   ┌───────────────┐
                   │ Google Gemini  │
                   │ API            │
                   └───────────────┘
```

### 2.2 Cross-Platform Design (macOS / Ubuntu / Windows)

3개 OS에서 동일한 사용자 경험을 제공하기 위해, OS별 차이를 추상화 계층에서 처리한다.

#### 2.2.1 Data Directory (앱 데이터 저장 경로)

모든 로컬 데이터(DB, config, logs)는 Tauri의 `app_data_dir()`이 반환하는 OS 표준 경로에 저장한다. 하드코딩된 경로(`~/.semantic-file-search/`)는 사용하지 않는다.

| OS | Data Directory | 예시 |
|----|---------------|------|
| macOS | `~/Library/Application Support/semantic-file-search/` | Tauri `app_data_dir()` |
| Ubuntu | `~/.local/share/semantic-file-search/` | XDG_DATA_HOME 준수 |
| Windows | `C:\Users\{user}\AppData\Roaming\semantic-file-search\` | %APPDATA% |

```
{app_data_dir}/
├── index.db              # SQLite (metadata + embedding + pending_files)
├── tantivy_index/        # Tantivy 전문 검색 인덱스 (segments)
├── config.json           # 사용자 설정 (API 키 제외)
└── logs/                 # tracing 로그 파일
    └── app.log
```

#### 2.2.2 File Path Handling

| 항목 | 구현 |
|------|------|
| 경로 표현 | `std::path::PathBuf` 사용 (OS별 구분자 자동 처리) |
| DB 저장 | `file_path`를 항상 **Unix 스타일 (`/`)로 정규화**하여 저장 — 인덱스 이식성 확보 |
| UI 표시 | 프론트엔드에 전달 시 OS 네이티브 경로로 변환 (`path.display()`) |
| 대소문자 | macOS/Windows는 case-insensitive, Ubuntu는 case-sensitive — 경로 비교 시 OS에 따라 분기 |
| 경로 길이 | Windows MAX_PATH(260자) 제한 대응: long path prefix (`\\?\`) 자동 추가 |

```rust
// Rust에서 크로스플랫폼 경로 처리
use std::path::PathBuf;
use tauri::api::path::app_data_dir;

fn normalize_path(path: &std::path::Path) -> String {
    // DB 저장용: Unix 스타일로 정규화
    path.to_string_lossy().replace('\\', "/")
}
```

#### 2.2.3 Default Exclude Directories

전체 드라이브 스캔 모드에서 기본 제외할 OS별 시스템 디렉토리:

| OS | 제외 디렉토리 |
|----|-------------|
| macOS | `/System`, `/Library`, `/private`, `/Volumes` (외장 드라이브), `~/Library`, `.Trash` |
| Ubuntu | `/proc`, `/sys`, `/dev`, `/run`, `/snap`, `/boot`, `/lost+found` |
| Windows | `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`, `C:\ProgramData`, `$Recycle.Bin` |
| 공통 | `.git`, `node_modules`, `.cache`, `__pycache__`, `.Trash*`, `Thumbs.db`, `.DS_Store` |

#### 2.2.4 File Watcher (notify crate) OS별 백엔드

| OS | Backend | 주의사항 |
|----|---------|---------|
| macOS | FSEvents | 안정적, 디렉토리 단위 감시 |
| Ubuntu | inotify | **watch 수 제한** — 기본값 8,192개. 대량 디렉토리 감시 시 `fs.inotify.max_user_watches` 증가 안내 필요 |
| Windows | ReadDirectoryChangesW | 안정적, 네트워크 드라이브 감시 시 제한 있음 |

Ubuntu에서 inotify watch 제한 초과 시 사용자에게 다음 안내 메시지를 표시:
```
감시 대상 디렉토리가 많아 시스템 제한을 초과했습니다.
다음 명령어를 실행한 후 앱을 재시작하세요:
echo fs.inotify.max_user_watches=524288 | sudo tee -a /etc/sysctl.conf && sudo sysctl -p
```

#### 2.2.5 System Tray / Menu Bar

| OS | 동작 |
|----|------|
| macOS | 메뉴 바 아이콘 (상단 우측) — 앱 창 닫아도 백그라운드 유지 |
| Ubuntu | AppIndicator (libappindicator3 필요) — 데스크톱 환경에 따라 표시 여부 다름 |
| Windows | 시스템 트레이 아이콘 (하단 우측) — 앱 창 닫아도 백그라운드 유지 |

#### 2.2.6 Open File in Default App

Tauri의 `tauri::api::shell::open()`을 사용하며, 내부적으로 OS별 명령을 호출:

| OS | 내부 호출 |
|----|----------|
| macOS | `open {path}` |
| Ubuntu | `xdg-open {path}` |
| Windows | `start "" "{path}"` |

#### 2.2.7 Webview Runtime

| OS | Webview | 설치 요구사항 |
|----|---------|-------------|
| macOS | WebKit (WKWebView) | 시스템 내장 — 추가 설치 없음 |
| Ubuntu | WebKitGTK | `libwebkit2gtk-4.1-dev` 패키지 필요 (설치 안내 제공) |
| Windows | WebView2 (Edge) | Windows 10/11에 기본 포함. 미설치 시 앱 첫 실행 시 자동 설치 (Tauri의 `webview_install_mode: downloadBootstrapper`) |

#### 2.2.8 OS Keychain Fallback

| OS | Primary Backend | Fallback |
|----|----------------|----------|
| macOS | Keychain Services | — (항상 사용 가능) |
| Ubuntu | Secret Service (GNOME Keyring) | Secret Service 미설치 시 → 로컬 암호화 파일 (`{app_data_dir}/credentials.enc`) + 사용자 경고 |
| Windows | Credential Manager | — (항상 사용 가능) |

Ubuntu 서버/최소 설치 환경에서 GNOME Keyring이 없을 수 있으므로, `keyring` crate 실패 시 AES-256 암호화 파일 fallback을 구현한다.

### 2.3 Component Responsibilities

| Component | Role |
|-----------|------|
| File Crawler | Recursive directory traversal, change detection (`notify` crate) |
| anytomd | Convert DOCX/PPTX/XLSX/CSV → Markdown text + embedded image extraction (pure Rust crate, `cargo add anytomd`) |
| Gemini API Client | Send text/PDF/images to Gemini → get summary, keywords, embeddings |
| Local Index DB | Store file metadata + summaries + embedding vectors in SQLite |
| Tantivy Index | Full-text search index — summary, keywords, file name에 대한 키워드/구문 검색 |
| Vector Search Engine | Cosine similarity between query embedding and document embeddings |
| Hybrid Ranker | Tantivy 점수 + Vector 점수를 가중 합산하여 최종 순위 결정 |
| Tauri Frontend | Desktop GUI (HTML/CSS/JS) for search and settings |

### 2.3 Document Processing by Format

| Input Format | Processing Pipeline |
|-------------|-------------------|
| PDF | Upload to Gemini Files API → analyze directly |
| DOCX | anytomd → Markdown text + image extraction → Gemini generateContent |
| PPTX | anytomd → Markdown text + image extraction → Gemini generateContent |
| XLSX | anytomd → Markdown text → Gemini generateContent (이미지 포함 시 자동 추출) |
| TXT, MD, CSV | Read file directly → Gemini generateContent |
| Images (JPG/PNG) | Upload to Gemini Files API → analyze via vision |

---

## 3. Tech Stack

### 3.1 Desktop App (Rust + Tauri)

| Item | Technology | Notes |
|------|-----------|-------|
| Framework | **Tauri v2** | Cross-platform desktop app (macOS, Ubuntu Linux, Windows) |
| Language (Backend) | **Rust** | Tauri backend, file operations, hybrid search |
| Language (Frontend) | **TypeScript + React** (or Svelte) | Tauri webview UI |
| Document Conversion | `anytomd` | Pure Rust DOCX/PPTX/XLSX/CSV/JSON → Markdown + image extraction |
| File Watching | `notify` crate | Cross-platform filesystem events |
| HTTP Client | `reqwest` | Gemini API communication |
| Local DB | `rusqlite` (SQLite) | Index storage |
| Full-Text Search | `tantivy` | Rust-native 전문 검색 엔진 (Lucene급), 키워드/구문 검색 |
| Vector Search | Custom implementation (cosine similarity) | 의미 기반 검색, brute-force for <100K files |
| Async Runtime | `tokio` | Parallel processing, file watching |
| Serialization | `serde` + `serde_json` | API communication |
| File Hashing | `blake3` | Fast content-based change detection |
| Error Handling | `thiserror` | Structured error types with derive macro |
| Logging | `tracing` + `tracing-subscriber` | Structured async-aware logging |
| Keychain | `keyring` | Cross-platform OS credential storage (macOS Keychain, Windows Credential Manager, Ubuntu Secret Service) |
| Platform Dirs | `dirs` | OS 표준 디렉토리 경로 감지 (Documents, Desktop, Home 등) |

### 3.2 Document Conversion (Pure Rust)

| Item | Technology | Notes |
|------|-----------|-------|
| Converter | **[anytomd](https://crates.io/crates/anytomd)** | Pure Rust reimplementation of Microsoft MarkItDown. DOCX/PPTX/XLSX/CSV/JSON → Markdown + image extraction |
| Integration | `cargo add anytomd` | Rust crate — no sidecar, no Python, no external runtime |
| Formats | DOCX, PPTX, XLSX, CSV, JSON, Plain Text | OOXML direct parsing via `zip` + `quick-xml`, XLSX via `calamine` |

### 3.3 Why anytomd over MarkItDown (Python) / LibreOffice

| Concern | anytomd (Rust) | MarkItDown (Python) | LibreOffice |
|---------|---------------|---------------------|-------------|
| Install size | 0 (compiled into binary) | ~50MB (Python + deps) | ~300-500MB |
| Runtime dependency | None | Python 3.12+ | LibreOffice process |
| Thread safety | Native Rust safety | GIL-bound | NOT thread-safe |
| Cross-platform | Compiled with Tauri — zero bundling issues | PyInstaller per OS — known issues | Portable only on Windows |
| IPC overhead | None (in-process function call) | Sidecar JSON IPC | Subprocess + pipe |
| Output format | Markdown + image binary extraction | Markdown (LLM-optimized) | PDF (requires Gemini upload) |
| CJK/emoji support | Full Unicode preservation | Inherent (reads XML) | Requires font packages |
| Image extraction | `ConversionResult.images` — binary + filename | Custom ZIP extraction script | N/A |
| Maintenance | `cargo update` | pip update + PyInstaller rebuild | System package management |
| Test coverage | 339+ unit/integration tests, CI on 3 OS | Upstream maintained | N/A |

**결정: anytomd 사용 (Python Sidecar 제거)**

- anytomd는 MarkItDown의 순수 Rust 재구현으로, 테이블/목록/서식/이미지를 LLM에 최적화된 Markdown으로 변환
- Python sidecar가 불필요하므로 PyInstaller 번들링, 크로스플랫폼 빌드 문제, IPC 오버헤드가 모두 제거됨
- DOCX/PPTX의 이미지 추출이 내장되어 있어 별도 ZIP 파싱 코드 불필요
- 같은 저자가 개발하므로 semantic-file-search에 필요한 기능을 직접 추가/수정 가능

### 3.5 AI Model Details

#### Gemini 3.0 Flash (Document Analysis)

| Property | Value |
|----------|-------|
| Model ID | `gemini-3-flash-preview` (after GA: `gemini-3-flash`) |
| Context Window | 1,048,576 tokens (1M) |
| Max Output | 65,536 tokens (64K) |
| Tokens per PDF Page | ~258 tokens (medium resolution) |
| Max PDF Size | 50 MB or 1,000 pages |
| Scanned Doc OCR | Natively supported (vision-based) |
| Media Resolution | Configurable: `low` / `medium` (recommended) / `high` |
| Input Price | $0.50 / 1M tokens |
| Output Price | $3.00 / 1M tokens |
| Batch Input Price | $0.25 / 1M tokens (50% discount) |
| Batch Output Price | $1.50 / 1M tokens (50% discount) |

#### Gemini Embedding (Vector Generation)

| Property | Value |
|----------|-------|
| Model ID | `gemini-embedding-001` |
| Output Dimensions | 128 – 3,072 (variable; recommended: 1536) |
| Input Limit | 2,048 tokens per input |
| Task Types | `RETRIEVAL_DOCUMENT` (indexing), `RETRIEVAL_QUERY` (search) |
| Price | $0.15 / 1M tokens |
| Multilingual | 100+ languages supported (including Korean) |
| Technique | Matryoshka Representation Learning (MRL) — truncation-friendly |

#### API Endpoints

```
# Document analysis (summary / keyword extraction)
POST https://generativelanguage.googleapis.com/v1beta/models/gemini-3-flash-preview:generateContent

# Embedding generation
POST https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent

# File upload (Gemini Files API — for PDF and images)
POST https://generativelanguage.googleapis.com/upload/v1beta/files

# Authentication: x-goog-api-key header
```

---

## 4. Core Features

### 4.1 File Indexing Pipeline

```
1. Crawl:     Recursively traverse specified directories
2. Detect:    Compare file hash (blake3) + modification time against index
3. Convert:   (if DOCX/PPTX/XLSX/CSV) anytomd::convert_file() → ConversionResult { markdown, images }
              (if PDF/image) keep as-is for Gemini Files API upload
              (if TXT/MD) read file content directly
4. Analyze:   Send to Gemini API → extract summary + keywords
              (DOCX/PPTX: markdown text + ConversionResult.images를 함께 전송)
5. Embed:     Summary text → Gemini Embedding API → 1536-dim vector
6. FT-Index:  Tantivy에 문서 등록 (summary + keywords + file_name → full-text index)
7. Store:     Save to local SQLite index (metadata + embedding)
```

### 4.2 Hybrid Search Flow

검색은 **키워드 검색(Tantivy)** 과 **의미 검색(Vector)** 을 동시에 수행하고 점수를 합산한다.

```
1. User input:    "2024년 Q3 매출 보고서"
                         │
              ┌──────────┴──────────┐
              ▼                     ▼
2a. Tantivy 검색              2b. Vector 검색
    쿼리 텍스트 그대로             쿼리 → Gemini Embedding API
    full-text search               cosine similarity
    → keyword_score (BM25)         → vector_score (0.0~1.0)
              │                     │
              └──────────┬──────────┘
                         ▼
3. Hybrid Ranking:
    final_score = α × normalize(keyword_score) + (1-α) × vector_score
    α = 0.4 (기본값, 사용자 설정 가능)
                         │
                         ▼
4. Display:       상위 N개 결과 (file path + summary + score + match type)
```

**검색 모드별 동작:**

| 쿼리 유형 | 예시 | Tantivy 기여 | Vector 기여 |
|-----------|------|-------------|-------------|
| 정확한 키워드 | "김철수 계약서 2024-03" | 높음 (BM25 매칭) | 낮음 |
| 의미 기반 | "지난 분기 실적 자료" | 낮음 | 높음 (의미 유사도) |
| 혼합 | "Q3 revenue Excel" | 중간 | 중간 |

**오프라인 검색 지원:**
- Gemini API 호출 불가 시(네트워크 끊김), Tantivy 키워드 검색만으로 결과 반환 (degraded mode)
- 사용자에게 "키워드 검색만 사용 중" 안내 표시

### 4.3 anytomd Document Conversion + Image Extraction

anytomd는 Rust crate로 Tauri 바이너리에 직접 컴파일된다. Python sidecar, IPC, 별도 프로세스가 불필요하다.

DOCX/PPTX 처리 시:
1. `anytomd::convert_file(path)` 또는 `anytomd::convert_bytes(data, ext)` 호출
2. 내부적으로 OOXML ZIP 파싱 → Markdown 변환 + 이미지 바이너리 추출을 동시 수행
3. 결과: `ConversionResult { markdown, images, title, warnings }`
4. `images: Vec<ImageData>` — 각 이미지의 파일명 + 바이너리 데이터
5. Gemini에 전송 시 markdown text + images를 함께 전달

```rust
use anytomd::{convert_file, ConversionOptions};

let result = convert_file("document.docx", &ConversionOptions::default())?;
// result.markdown  — LLM-optimized Markdown text
// result.images    — Vec<ImageData> { filename, data: Vec<u8> }
// result.title     — Option<String> (document title if present)
// result.warnings  — Vec<ConversionWarning> (best-effort parse warnings)
```

- In-process Rust function call — no IPC, no sidecar process management
- No network required for conversion
- 이미지 추출은 OOXML의 Relationship ID(rId) → media 파일 경로 매핑을 내부적으로 처리
- CSV, JSON, Plain Text도 `convert_file()`로 통합 처리 가능

### 4.4 Indexing Concurrency Model

인덱싱 파이프라인은 tokio 기반 비동기 처리로 구현하며, 다음과 같은 동시성 제어를 적용한다.

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────┐
│  File Queue   │────▶│  Convert     │────▶│  Gemini API  │────▶│  DB Write │
│  (unbounded   │     │  Worker Pool │     │  Worker Pool │     │  (single  │
│   channel)    │     │  (rayon/     │     │  (semaphore) │     │   writer) │
│               │     │   blocking)  │     │              │     │          │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────┘
       ▲                                                              │
       │              File Watcher (notify)                          │
       └──────────── debounce 2s ◀───────────────────────────────────┘
```

| 단계 | 동시성 제어 | 구현 |
|------|------------|------|
| File Queue | `tokio::sync::mpsc` unbounded channel | 크롤러/파일워처가 발견한 파일 경로를 큐에 적재 |
| Convert (anytomd) | `tokio::task::spawn_blocking` × 4 | anytomd 변환은 CPU-bound (ZIP/XML 파싱) — blocking task pool에서 실행 |
| Gemini API 호출 | `tokio::sync::Semaphore` (Free: 5, Paid: 50 permits) | Rate limit 티어에 맞춰 동시 요청 수 제한 |
| Retry | Exponential backoff (1s → 2s → 4s → ... 최대 60s) | 429/5xx 응답 시 자동 재시도, 최대 5회 |
| DB Write | `tokio::sync::RwLock` on DB connection | 인덱싱(write)과 검색(read)이 동시에 가능하도록 RwLock 사용 |
| 검색 요청 | 인덱싱 중에도 검색 가능 (read lock) | 검색은 DB의 현재 상태 기준으로 즉시 응답 |

**인덱싱 중단/재개:**
- 앱 종료 시 현재 큐 상태를 SQLite에 `pending_files` 테이블로 저장
- 앱 재시작 시 pending 파일부터 처리 재개
- 사용자가 인덱싱을 수동으로 일시정지/재개할 수 있는 UI 제공

### 4.5 File Watching (Real-Time)

```rust
// File change detection via notify crate
// Handle Create, Modify, Remove events
// Apply debouncing (2 seconds) to prevent redundant processing
```

---

## 5. Data Model

### 5.1 Local SQLite Schema

```sql
CREATE TABLE files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path     TEXT NOT NULL UNIQUE,
    file_name     TEXT NOT NULL,
    file_ext      TEXT NOT NULL,
    file_size     INTEGER NOT NULL,
    file_hash     TEXT NOT NULL,          -- blake3 hash
    modified_at   TEXT NOT NULL,           -- ISO 8601
    indexed_at    TEXT NOT NULL,           -- ISO 8601
    summary       TEXT NOT NULL,           -- Gemini-generated summary
    keywords      TEXT NOT NULL,           -- JSON array ["revenue", "Q3", ...]
    embedding     BLOB NOT NULL            -- 1536-dim f32 vector (6,144 bytes = 1536 × 4)
);

CREATE INDEX idx_files_hash ON files(file_hash);
CREATE INDEX idx_files_ext ON files(file_ext);
CREATE INDEX idx_files_path ON files(file_path);
```

### 5.2 Tantivy Full-Text Index Schema

Tantivy 인덱스는 SQLite와 별도로 `{app_data_dir}/tantivy_index/` 디렉토리에 저장된다.

```rust
// Tantivy schema definition
let mut schema_builder = Schema::builder();

// 검색 대상 필드 (tokenized, indexed)
schema_builder.add_text_field("summary", TEXT | STORED);     // Gemini 생성 요약
schema_builder.add_text_field("keywords", TEXT | STORED);    // Gemini 추출 키워드
schema_builder.add_text_field("file_name", TEXT | STORED);   // 파일명 (확장자 포함)
schema_builder.add_text_field("file_path", STRING | STORED); // 전체 경로 (토큰화 안 함)

// 필터/정렬용 필드
schema_builder.add_text_field("file_ext", STRING | STORED);  // 확장자 필터
schema_builder.add_date_field("modified_at", INDEXED | STORED); // 날짜 범위 필터
schema_builder.add_u64_field("file_size", INDEXED | STORED);    // 크기 필터

// SQLite 연결용
schema_builder.add_u64_field("db_id", STORED);               // SQLite files.id 참조
```

**Tokenizer 설정:**

| 언어 | Tokenizer | 비고 |
|------|-----------|------|
| 영어 | Tantivy 기본 (`SimpleTokenizer` + `LowerCaser` + `Stemmer`) | 내장 |
| 한국어 | `lindera` tokenizer (MeCab 사전 기반) | `tantivy-jieba` 대안도 가능, `lindera-tantivy` crate 사용 |
| CJK 공통 | `lindera` + UniDic/IPAdic | 한국어/일본어/중국어 형태소 분석 |

```rust
// 한국어 + 영어 혼용 텍스트를 처리하는 multi-language tokenizer
// lindera의 한국어 사전(ko-dic)을 사용하여 형태소 분석
let tokenizer = LinderaTokenizer::with_config(LinderaConfig {
    mode: Mode::Normal,
    dict: DictType::KoDic,  // 한국어 사전
});
index.tokenizers().register("korean", tokenizer);
```

**인덱스 저장 구조:**
```
{app_data_dir}/
├── index.db                # SQLite (metadata + embedding)
├── tantivy_index/          # Tantivy 전문 검색 인덱스
│   ├── meta.json
│   └── segments/
└── ...
```

### 5.3 Configuration

```json
// {app_data_dir}/config.json (Section 2.2.1 참조)
// API 키는 OS keychain에 저장 (Section 7.4 참조)
{
  "watch_directories": [],
  "supported_extensions": [".pdf", ".docx", ".pptx", ".xlsx", ".txt", ".md", ".csv", ".json", ".jpg", ".png"],
  "embedding_dimensions": 1536,
  "gemini_model": "gemini-3-flash-preview",
  "search_alpha": 0.4
}
```

**`watch_directories` OS별 기본값 예시:**
- macOS: `["/Users/{user}/Documents", "/Users/{user}/Desktop"]`
- Ubuntu: `["/home/{user}/Documents", "/home/{user}/Desktop"]`
- Windows: `["C:\\Users\\{user}\\Documents", "C:\\Users\\{user}\\Desktop"]`

앱 첫 실행 시 `dirs::document_dir()`, `dirs::desktop_dir()` 등 Rust `dirs` crate으로 OS 표준 경로를 자동 감지하여 기본값을 제안한다.

---

## 6. Gemini API Integration Details

### 6.1 Document Analysis Prompt

```
Analyze the following document and respond in JSON.

1. summary: Summarize the document content in 2-3 sentences
2. keywords: Array of 5-10 key terms
3. language: Primary language of the document (ISO 639-1 code)
4. doc_type: Document type (report / contract / presentation / spreadsheet / memo / other)

Response format:
{
  "summary": "...",
  "keywords": ["...", "..."],
  "language": "en",
  "doc_type": "report"
}
```

### 6.2 API Call Patterns (Rust)

```rust
// For PDF and images: use Gemini Files API
// 1. Upload file → get file URI
// 2. Send URI + prompt to generateContent

// For DOCX/PPTX/XLSX/CSV (anytomd-converted):
// 1. anytomd::convert_file() → ConversionResult { markdown, images }
// 2. Send markdown text + images to generateContent (inline image parts)

// For plain text (TXT/MD):
// 1. Read file content directly → send to generateContent

// For embeddings:
// POST to embedContent with task_type RETRIEVAL_DOCUMENT or RETRIEVAL_QUERY
```

### 6.3 Rate Limit Handling

| Tier | RPM | TPM | RPD |
|------|-----|-----|-----|
| Free | ~10 | 250,000 | ~1,000 |
| Tier 1 (Paid) | ~300 | 1,000,000 | ~1,000 |
| Tier 2 | ~1,000 | 2,000,000 | ~10,000 |

- Use **Batch API** for bulk indexing (50% discount, async processing)
- Implement exponential backoff retry logic
- Files API uploads have no separate rate limit

### 6.4 Cost Estimation (Single PC)

단일 PC에서 개인 사용자가 로컬 파일을 인덱싱할 때의 Gemini API 비용 추정:

| 파일 수 | 총 페이지 (평균 5p/file) | 초기 인덱싱 (1회) | 월간 유지 (변경분) |
|---------|------------------------|-------------------|-------------------|
| 500 | 2,500 pages | ~$0.1 | ~$0.01 |
| 5,000 | 25,000 pages | ~$1 | ~$0.1 |
| 50,000 | 250,000 pages | ~$10 | ~$1 |

- 모든 처리는 사용자의 로컬 PC에서 실행 — 서버/클라우드 인프라 비용 없음
- 비용은 오직 Gemini API 사용량에 비례
- Batch API 사용 시 50% 할인 적용 가능

---

## 7. Rust Implementation Design

### 7.1 Error Type Hierarchy

`thiserror`를 사용하여 구조화된 에러 타입을 정의한다. 모든 에러는 최종적으로 `AppError`로 통합되어 프론트엔드에 전달된다.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    // Gemini API 관련
    #[error("Gemini API error: {message} (status: {status_code})")]
    GeminiApi { status_code: u16, message: String },

    #[error("Gemini rate limit exceeded, retry after {retry_after_secs}s")]
    GeminiRateLimit { retry_after_secs: u64 },

    #[error("Invalid API key")]
    InvalidApiKey,

    // 파일 처리 관련
    #[error("File I/O error: {path}")]
    FileIo { path: String, #[source] source: std::io::Error },

    #[error("Unsupported file format: {extension}")]
    UnsupportedFormat { extension: String },

    // 문서 변환 관련 (anytomd)
    #[error("Document conversion failed: {path}")]
    ConversionFailed { path: String, #[source] source: anytomd::ConvertError },

    // DB 관련
    #[error("Database error")]
    Database(#[from] rusqlite::Error),

    // 검색 관련
    #[error("Embedding generation failed for query")]
    EmbeddingFailed { query: String },
}
```

**프론트엔드 전달 포맷:**

Tauri IPC를 통해 프론트엔드에 전달 시 직렬화 가능한 구조로 변환한다.

```rust
#[derive(serde::Serialize)]
pub struct ErrorResponse {
    pub code: String,          // "GEMINI_RATE_LIMIT", "FILE_IO", etc.
    pub message: String,       // 사용자에게 표시할 메시지
    pub recoverable: bool,     // 자동 재시도 가능 여부
}

impl From<AppError> for ErrorResponse { ... }
```

### 7.2 Tauri IPC Command Definitions

프론트엔드↔Rust 백엔드 간 통신은 `#[tauri::command]`로 정의한다.

```rust
// === 검색 ===
#[tauri::command]
async fn search_files(
    query: String,
    limit: Option<usize>,
    mode: Option<SearchMode>,       // Hybrid (default), KeywordOnly, VectorOnly
    alpha: Option<f32>,             // 키워드 가중치 (0.0~1.0, default 0.4)
) -> Result<SearchResponse, ErrorResponse>;

#[derive(serde::Deserialize)]
enum SearchMode {
    Hybrid,       // Tantivy + Vector (기본)
    KeywordOnly,  // Tantivy만 (오프라인 시 자동 전환)
    VectorOnly,   // Vector만
}

#[derive(serde::Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    mode_used: SearchMode,          // 실제 사용된 모드 (오프라인 시 KeywordOnly로 전환됨)
    query_time_ms: u64,             // 검색 소요 시간
}

#[derive(serde::Serialize)]
struct SearchResult {
    file_path: String,
    file_name: String,
    file_ext: String,
    summary: String,
    keywords: Vec<String>,
    final_score: f32,               // 최종 하이브리드 점수 (0.0 ~ 1.0)
    keyword_score: Option<f32>,     // Tantivy BM25 점수 (정규화)
    vector_score: Option<f32>,      // Cosine similarity 점수
    modified_at: String,            // ISO 8601
}

// === 인덱싱 ===
#[tauri::command]
async fn start_indexing(directories: Vec<String>) -> Result<(), ErrorResponse>;

#[tauri::command]
async fn pause_indexing() -> Result<(), ErrorResponse>;

#[tauri::command]
async fn resume_indexing() -> Result<(), ErrorResponse>;

#[tauri::command]
async fn get_indexing_status() -> Result<IndexingStatus, ErrorResponse>;

#[derive(serde::Serialize)]
struct IndexingStatus {
    state: IndexingState,           // Idle, Running, Paused
    total_files: usize,
    indexed_files: usize,
    failed_files: usize,
    current_file: Option<String>,   // 현재 처리 중인 파일 경로
}

// === 설정 ===
#[tauri::command]
async fn validate_api_key(key: String) -> Result<bool, ErrorResponse>;

#[tauri::command]
async fn save_api_key(key: String) -> Result<(), ErrorResponse>;  // OS keychain에 저장

#[tauri::command]
async fn get_config() -> Result<AppConfig, ErrorResponse>;

#[tauri::command]
async fn update_config(config: AppConfig) -> Result<(), ErrorResponse>;

// === 파일 ===
#[tauri::command]
async fn open_file(file_path: String) -> Result<(), ErrorResponse>;  // 기본 앱으로 열기

#[tauri::command]
async fn get_indexed_stats() -> Result<IndexedStats, ErrorResponse>;

#[derive(serde::Serialize)]
struct IndexedStats {
    total_files: usize,
    by_extension: HashMap<String, usize>,  // {".pdf": 120, ".docx": 45, ...}
    total_size_bytes: u64,
    last_indexed_at: Option<String>,
}
```

### 7.3 Hybrid Search Engine Design

#### 7.3.1 Tantivy Full-Text Search

Tantivy는 Rust 네이티브 전문 검색 엔진으로, 이 앱에서 Rust를 사용하는 핵심 이유 중 하나다.

```rust
use tantivy::{Index, collector::TopDocs, query::QueryParser};

fn keyword_search(index: &Index, query_text: &str, limit: usize) -> Vec<(f32, u64)> {
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();
    let query_parser = QueryParser::for_index(index, vec![
        summary_field, keywords_field, file_name_field
    ]);
    let query = query_parser.parse_query(query_text).unwrap();
    let top_docs = searcher.search(&query, &TopDocs::with_limit(limit)).unwrap();
    // (BM25 score, db_id) 반환
    top_docs.into_iter().map(|(score, doc_addr)| {
        let doc = searcher.doc(doc_addr).unwrap();
        let db_id = doc.get_first(db_id_field).unwrap().as_u64().unwrap();
        (score, db_id)
    }).collect()
}
```

**Tantivy가 Python 대안보다 나은 이유:**

| 항목 | Tantivy (Rust) | Whoosh (Python) | SQLite FTS5 |
|------|---------------|-----------------|-------------|
| 검색 속도 (100K docs) | ~1ms | ~50ms | ~10ms |
| 인덱싱 속도 | 매우 빠름 | 느림 | 중간 |
| 한국어 토큰화 | lindera (MeCab) | 없음 (별도 구현) | 없음 (별도 구현) |
| 메모리 사용 | mmap 기반, 저메모리 | 전체 로드 | DB에 종속 |
| BM25 랭킹 | 내장 | 내장 | 제한적 |
| 실시간 인크리멘탈 | 지원 | 지원 | 지원 |

#### 7.3.2 Vector Search

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}
```

**Vector Search 최적화 경로:**

| 파일 수 | 전략 | 구현 |
|---------|------|------|
| < 10K | Brute-force (단일 스레드) | `Vec<f32>` 순회 |
| 10K ~ 50K | Brute-force + 병렬화 | `rayon::par_iter()` |
| 50K ~ 100K | SIMD + 병렬화 | f32 SIMD 가속 |
| > 100K (Phase 3) | ANN 인덱스 | `usearch` crate |

#### 7.3.3 Hybrid Score Fusion

두 검색 결과를 합산하는 점수 정규화 + 가중 합산 로직:

```rust
/// Hybrid search: Tantivy(keyword) + Vector(semantic) 결과를 합산
fn hybrid_search(
    query: &str,
    query_embedding: &[f32],
    alpha: f32,  // keyword 가중치 (0.0~1.0)
    limit: usize,
) -> Vec<SearchResult> {
    // 1. Tantivy 검색 (BM25 score)
    let keyword_results = keyword_search(query, limit * 2);

    // 2. Vector 검색 (cosine similarity)
    let vector_results = vector_search(query_embedding, limit * 2);

    // 3. BM25 점수를 0.0~1.0으로 정규화 (min-max normalization)
    let keyword_normalized = normalize_scores(&keyword_results);

    // 4. 점수 합산: final = α × keyword + (1-α) × vector
    let merged = merge_and_rank(keyword_normalized, vector_results, alpha);

    merged.into_iter().take(limit).collect()
}
```

**α (alpha) 가중치 가이드:**

| α 값 | 키워드 비중 | 의미 비중 | 적합한 쿼리 |
|------|-----------|----------|------------|
| 0.0 | 0% | 100% | "지난 분기 실적 자료" |
| 0.4 (기본) | 40% | 60% | 일반적인 혼합 쿼리 |
| 0.7 | 70% | 30% | "김철수 계약서 2024-03" |
| 1.0 | 100% | 0% | 정확한 키워드 검색 (오프라인 모드) |

### 7.4 OS Keychain Integration

API 키는 설정 파일이 아닌 OS 키체인에 저장하여 보안을 확보한다. (OS별 fallback 정책은 Section 2.2.8 참조)

| OS | Backend | Crate |
|----|---------|-------|
| macOS | Keychain Services | `keyring` |
| Windows | Credential Manager | `keyring` |
| Ubuntu | Secret Service (GNOME Keyring / KWallet) | `keyring` + fallback |

```rust
use keyring::Entry;

const SERVICE_NAME: &str = "semantic-file-search";
const KEY_NAME: &str = "gemini-api-key";

fn store_api_key(key: &str) -> Result<(), AppError> {
    // 1차: OS keychain 시도
    match Entry::new(SERVICE_NAME, KEY_NAME)?.set_password(key) {
        Ok(()) => Ok(()),
        Err(_) => {
            // 2차 (Ubuntu fallback): 로컬 암호화 파일
            store_api_key_encrypted(key)
        }
    }
}
```

---

## 8. User Experience

### 8.1 Initial Setup (First Launch)

앱 첫 실행 시 사용자에게 다음을 안내:

1. **Gemini API Key 입력** — 앱이 동작하려면 Google Gemini API 키가 필요
   - 설정 화면에서 API 키 입력란 제공
   - API 키 발급 방법 안내 링크 포함 (https://aistudio.google.com/apikey)
   - 입력 후 유효성 검증 (Gemini API health check 호출)
   - 유효하지 않은 키 → 에러 메시지와 재입력 유도
   - API 키는 OS keychain에 저장 (macOS Keychain, Windows Credential Manager, Ubuntu Secret Service)
2. **검색 대상 디렉토리 선택** — 두 가지 모드 제공:
   - **디렉토리 선택 모드** (기본) — 사용자가 지정한 폴더의 모든 하위 파일을 재귀적으로 분석
   - **전체 드라이브 스캔 모드** — 시스템의 모든 드라이브/마운트 포인트를 탐색하여 지원되는 모든 파일을 분석. OS별 시스템 디렉토리는 기본 제외 (Section 2.2.3 참조). 파일 수가 매우 많을 수 있으므로 예상 소요 시간/비용 안내 후 사용자 확인 필요.
3. **인덱싱 시작** — 선택한 범위의 파일을 백그라운드에서 인덱싱

API 키가 설정되지 않으면 검색/인덱싱 기능이 비활성화되어야 함.

### 8.2 Desktop GUI (Tauri)

- Search bar (main screen) — type natural language query
- Results list with file path, summary, similarity score
- Click to open file in default application
- Settings page: API key 관리, directory selection, indexing status
- System tray: background file watching daemon

### 8.3 Supported File Formats

| Format | Extensions | Processing Method |
|--------|-----------|-------------------|
| PDF | `.pdf` | Gemini Files API → direct analysis |
| Word | `.docx` | anytomd → Markdown + images → Gemini |
| PowerPoint | `.pptx` | anytomd → Markdown + images → Gemini |
| Excel | `.xlsx` | anytomd → Markdown → Gemini |
| CSV | `.csv` | anytomd → Markdown table → Gemini |
| Text | `.txt`, `.md` | Read directly → Gemini |
| JSON | `.json` | anytomd → formatted text → Gemini |
| Images | `.jpg`, `.png` | Gemini Files API → vision analysis |

---

## 9. Project Structure

```
semantic-file-search/
├── src-tauri/                       # Rust backend (Tauri)
│   ├── Cargo.toml                   # anytomd dependency here
│   └── src/
│       ├── main.rs                  # Tauri entry point + IPC command registration
│       ├── commands.rs              # #[tauri::command] handler definitions
│       ├── crawler.rs               # File crawling / watching (notify)
│       ├── converter.rs             # anytomd wrapper — convert_file → ConversionResult
│       ├── gemini.rs                # Gemini API client (reqwest)
│       ├── search.rs                # Hybrid search orchestration (Tantivy + Vector)
│       ├── tantivy_index.rs         # Tantivy index build/query/tokenizer setup
│       ├── vector_search.rs         # Cosine similarity vector search
│       ├── db.rs                    # SQLite management (rusqlite)
│       ├── config.rs                # Configuration management
│       ├── keychain.rs              # OS keychain integration (keyring)
│       ├── pipeline.rs              # Indexing pipeline orchestration (channels, semaphore)
│       ├── error.rs                 # AppError type definitions (thiserror)
│       └── platform.rs             # OS-specific path normalization, exclude dirs, keychain fallback
│
├── src/                             # Frontend (React/Svelte)
│   ├── App.tsx
│   ├── components/
│   │   ├── SearchBar.tsx
│   │   ├── ResultList.tsx
│   │   └── Settings.tsx
│   └── ...
│
├── package.json                     # Frontend dependencies
└── tauri.conf.json                  # Tauri configuration
```

---

## 10. Milestones

### Phase 1: Foundation (MVP)
- [ ] Tauri project setup (Rust backend + frontend scaffold)
- [ ] 크로스플랫폼 기반 모듈 (`platform.rs`) — 경로 정규화, 데이터 디렉토리, 기본 제외 목록
- [ ] Rust error types (`AppError`) + logging (`tracing`) 기반 구축
- [ ] OS keychain 연동 (`keyring` crate) — API 키 저장/조회 + Ubuntu fallback
- [ ] anytomd 연동 (`converter.rs`) — `cargo add anytomd`, convert_file/convert_bytes 래퍼
- [ ] Gemini API client in Rust (summary + keywords + embedding)
- [ ] SQLite index DB implementation (`app_data_dir()` 경로에 저장)
- [ ] Tantivy 전문 검색 인덱스 구축 (schema + 한국어 tokenizer 설정)
- [ ] Indexing pipeline 구현 (channel + semaphore 기반 동시성, Tantivy 인덱싱 포함)
- [ ] File crawling + indexing pipeline (end-to-end)
- [ ] Hybrid search 구현 (Tantivy BM25 + Vector cosine similarity + score fusion)
- [ ] 오프라인 검색 fallback (Tantivy keyword-only mode)
- [ ] Tauri IPC commands 등록 (Section 7.2 정의 기준)
- [ ] Basic search UI (search bar + result list + 검색 모드 선택)
- [ ] Settings UI (API key, directory selection, OS 기본 경로 자동 감지)

### Phase 2: Production-Ready
- [ ] File change watching (`notify`-based background daemon)
- [ ] Incremental indexing (process only changed files)
- [ ] Search quality tuning (prompt optimization + α 가중치 튜닝)
- [ ] Rate limit handling + retry logic (exponential backoff)
- [ ] `rayon` 기반 벡터 검색 병렬화 (10K+ 파일 대응)
- [ ] Error handling / logging improvements
- [ ] System tray / menu bar integration (OS별 동작 — Section 2.2.5)
- [ ] Ubuntu inotify watch 제한 초과 시 안내 UI
- [ ] Cross-platform installer build:
  - macOS: DMG (Universal Binary — Apple Silicon + Intel)
  - Ubuntu: AppImage + `.deb` (libwebkit2gtk 의존성 포함)
  - Windows: NSIS installer (WebView2 bootstrapper 포함)

### Phase 3: Extensions
- [ ] Search filters (date, file type, directory) — Tantivy의 date/string 필드 활용
- [ ] Search history / favorites
- [ ] Multi-language UI
- [ ] SIMD 최적화 벡터 검색 또는 ANN 인덱스 도입 (100K+ 파일 대응)
- [ ] anytomd에 HTML/PDF 변환 추가 시 지원 포맷 확장
- [ ] HWP/HWPX support (별도 변환 필요 — MVP에서는 제외)

---

## 11. Risks and Mitigations

### 11.1 General Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Gemini API outage | Indexing blocked | Retry queue + `pending_files` 테이블에 저장하여 재시작 시 재개 |
| anytomd conversion failure | Specific files missing from index | anytomd는 best-effort 변환 — `ConversionResult.warnings`로 부분 실패 보고. 완전 실패 시 log + skip gracefully |
| Large file processing latency | Poor UX | Progress indicator + background processing + 인덱싱 일시정지/재개 UI |
| Gemini model ID change | API calls fail | Manage model ID via config |
| Gemini Files API 48-hour expiry | File deleted before analysis | Process immediately after upload |
| Scanned document OCR accuracy | Reduced search quality | Use Gemini vision `media_resolution: high` option |
| API key security | Key exposed in config file | OS keychain 저장 (`keyring` crate), config.json에 키 미포함 |
| anytomd 미지원 포맷 | PDF, HWP 등 변환 불가 | PDF/이미지는 Gemini Files API 직접 전송, 미지원 포맷은 skip + 사용자 안내 |
| 대용량 임베딩 메모리 사용 | 100K 파일 시 ~600MB 메모리 | Phase 3에서 mmap 기반 접근 또는 ANN 인덱스로 전환 |
| 인덱싱 중 앱 종료 | 작업 손실 | `pending_files` 테이블에 큐 상태 영속화, 재시작 시 자동 재개 |

### 11.2 Cross-Platform Risks

| Risk | 영향 OS | Impact | Mitigation |
|------|--------|--------|------------|
| Ubuntu inotify watch 제한 | Ubuntu | 대량 디렉토리 감시 실패 | 에러 감지 시 사용자에게 `sysctl` 설정 변경 안내 UI 표시 |
| Ubuntu에 GNOME Keyring 미설치 | Ubuntu | API 키 저장 실패 | 로컬 암호화 파일 fallback + 사용자 경고 (Section 2.2.8) |
| Ubuntu에 WebKitGTK 미설치 | Ubuntu | 앱 실행 불가 | `.deb` 패키지에 의존성 명시 + 설치 안내 문서 |
| Windows MAX_PATH 제한 | Windows | 긴 경로의 파일 인덱싱 실패 | long path prefix (`\\?\`) 자동 추가, manifest에 `longPathAware` 설정 |
| macOS code signing 미적용 | macOS | Gatekeeper가 앱 실행 차단 | Apple Developer ID로 코드 서명 + 공증 (Phase 2) |
| Windows Defender false positive | Windows | 앱이 멀웨어로 차단 | EV 코드 서명 인증서 사용 (Phase 2) |
| 파일 경로 대소문자 차이 | Ubuntu | 동일 이름 다른 케이스 파일 충돌 | DB에 경로 저장 시 원본 유지, 비교 시 OS 감지하여 분기 |

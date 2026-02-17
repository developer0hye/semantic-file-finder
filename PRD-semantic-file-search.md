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
- **No server required** — all processing runs locally, only Gemini API calls go to the network
- Cross-platform desktop app (Windows, macOS, Linux) via Tauri

---

## 2. Architecture

### 2.1 System Overview

```
┌─────────────────────────────────────────────────────────┐
│  Tauri Desktop App                                       │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐   │
│  │ File Crawler  │  │ Search Engine │  │ Local Index  │   │
│  │  (notify)    │  │(Vector Search)│  │   (SQLite)   │   │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘   │
│         │                 │                  │           │
│  ┌──────▼───────┐         │                  │           │
│  │ MarkItDown    │         │                  │           │
│  │ (Python       │         │                  │           │
│  │  Sidecar)     │         │                  │           │
│  │ DOCX/PPTX/   │         │                  │           │
│  │ XLSX → MD     │         │                  │           │
│  └──────┬───────┘         │                  │           │
│         │ Markdown text   │                  │           │
│         ▼                 │                  │           │
│  ┌──────────────────────────────────────┐    │           │
│  │ Gemini API Client (reqwest)           │    │           │
│  │  - Document analysis (summary/keywords)│    │           │
│  │  - Embedding generation (1536-dim)     │    │           │
│  └──────────────────┬───────────────────┘    │           │
│                     │ summary + embedding     │           │
│                     └────────────────────────▶│           │
└─────────────────────────────────────────────────────────┘
                      │
                      │ HTTPS (Gemini API only)
                      ▼
              ┌───────────────┐
              │ Google Gemini  │
              │ API            │
              └───────────────┘
```

### 2.2 Component Responsibilities

| Component | Role |
|-----------|------|
| File Crawler | Recursive directory traversal, change detection (`notify` crate) |
| MarkItDown Sidecar | Convert DOCX/PPTX/XLSX → Markdown text + embedded image extraction (Python sidecar bundled with app) |
| Gemini API Client | Send text/PDF/images to Gemini → get summary, keywords, embeddings |
| Local Index DB | Store file metadata + summaries + embedding vectors in SQLite |
| Search Engine | Cosine similarity between query embedding and document embeddings |
| Tauri Frontend | Desktop GUI (HTML/CSS/JS) for search and settings |

### 2.3 Document Processing by Format

| Input Format | Processing Pipeline |
|-------------|-------------------|
| PDF | Upload to Gemini Files API → analyze directly |
| DOCX | MarkItDown → Markdown text + ZIP image extraction → [IMAGE_N] marker matching → Gemini generateContent |
| PPTX | MarkItDown → Markdown text + ZIP image extraction → [IMAGE_N] marker matching → Gemini generateContent |
| XLSX | MarkItDown → Markdown text → Gemini generateContent (이미지 없음) |
| TXT, MD, CSV | Read file directly → Gemini generateContent |
| Images (JPG/PNG) | Upload to Gemini Files API → analyze via vision |

---

## 3. Tech Stack

### 3.1 Desktop App (Rust + Tauri)

| Item | Technology | Notes |
|------|-----------|-------|
| Framework | **Tauri v2** | Cross-platform desktop app (Windows, macOS, Linux) |
| Language (Backend) | **Rust** | Tauri backend, file operations, vector search |
| Language (Frontend) | **TypeScript + React** (or Svelte) | Tauri webview UI |
| File Watching | `notify` crate | Cross-platform filesystem events |
| HTTP Client | `reqwest` | Gemini API communication |
| Local DB | `rusqlite` (SQLite) | Index storage |
| Vector Search | Custom implementation (cosine similarity) | Brute-force is sufficient for <100K files |
| Async Runtime | `tokio` | Parallel processing, file watching |
| Serialization | `serde` + `serde_json` | API communication |
| File Hashing | `blake3` | Fast content-based change detection |

### 3.2 Document Conversion (Python Sidecar)

| Item | Technology | Notes |
|------|-----------|-------|
| Converter | **MarkItDown** (Microsoft) | DOCX/PPTX/XLSX → Markdown, no LibreOffice needed |
| Runtime | Python 3.12+ | Bundled as Tauri sidecar (PyInstaller or similar) |
| Package | `pip install markitdown` | Lightweight, pure Python |

### 3.3 Why MarkItDown over LibreOffice

| Concern | MarkItDown | LibreOffice |
|---------|-----------|-------------|
| Install size | ~50MB (Python + deps) | ~300-500MB |
| Thread safety | Safe | NOT thread-safe |
| Cross-platform bundling | PyInstaller → single binary | Portable only on Windows |
| Output format | Markdown (LLM-optimized) | PDF (requires Gemini Files API upload) |
| CJK support | Inherent (reads XML directly) | Requires font packages |
| Maintenance | pip update | System package management |

### 3.4 AI Model Details

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
3. Convert:   (if DOCX/PPTX) MarkItDown → Markdown text + ZIP에서 이미지 추출
                              → 텍스트 내 이미지 참조를 [IMAGE_N] 마커로 치환
              (if XLSX) MarkItDown → Markdown text
              (if PDF/image) keep as-is for Gemini Files API upload
              (if TXT/MD/CSV) read file content directly
4. Analyze:   Send to Gemini API → extract summary + keywords
              (DOCX/PPTX: [IMAGE_N] 마커 텍스트 + 이미지를 순서대로 매칭하여 전송)
5. Embed:     Summary text → Gemini Embedding API → 1536-dim vector
6. Store:     Save to local SQLite index
```

### 4.2 Search Flow

```
1. User input:    "Q3 revenue Excel file"
2. Query embed:   Send query text → Gemini Embedding API (task_type: RETRIEVAL_QUERY)
3. Vector search: Compute cosine similarity against local DB embeddings
4. Rank:          Sort results by similarity score (descending)
5. Display:       Show file path + summary + similarity score in GUI
```

### 4.3 MarkItDown Sidecar + Image Extraction

MarkItDown is bundled as a Python sidecar binary (compiled via PyInstaller) within the Tauri app.

DOCX/PPTX 처리 시:
1. MarkItDown으로 텍스트를 Markdown으로 변환
2. DOCX/PPTX ZIP 구조에서 이미지를 등장 순서대로 추출 (rels XML 파싱)
3. Markdown 내 `![alt](image.png)` 참조를 `[IMAGE_N]` 마커로 치환
4. Gemini에 전송 시 `[IMAGE_N]:` 라벨 + 이미지 바이너리를 순서대로 매칭

```python
# Sidecar script (convert.py)
# 텍스트 추출 + 이미지 추출 + 마커 매칭을 수행
# Input: file path
# Output: JSON { "text_content": "...[IMAGE_1]...", "images": ["base64...", ...] }
```

- Tauri invokes the sidecar via `Command::new_sidecar()`
- No network required for conversion
- 이미지 매칭은 OOXML의 Relationship ID(rId) → media 파일 경로 매핑을 활용

### 4.4 File Watching (Real-Time)

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
    embedding     BLOB NOT NULL            -- 1536-dim f32 vector (3,072 bytes)
);

CREATE INDEX idx_files_hash ON files(file_hash);
CREATE INDEX idx_files_ext ON files(file_ext);
CREATE INDEX idx_files_path ON files(file_path);
```

### 5.2 Configuration

```json
// ~/.semantic-file-search/config.json
{
  "gemini_api_key": "...",
  "watch_directories": ["~/Documents", "~/Desktop"],
  "supported_extensions": [".pdf", ".docx", ".pptx", ".xlsx", ".txt", ".md", ".csv", ".jpg", ".png"],
  "embedding_dimensions": 1536,
  "gemini_model": "gemini-3-flash-preview"
}
```

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

// For Markdown text (converted by MarkItDown) and plain text:
// 1. Send text content + prompt directly to generateContent (no file upload needed)

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

### 6.4 Cost Estimation

| Scenario | Files | Avg 5 pages | Indexing Cost (one-time) | Monthly Maintenance |
|----------|-------|-------------|------------------------|---------------------|
| Personal | 500 | 2,500 pages | ~$0.1 | ~$0.01 |
| Small Team | 5,000 | 25,000 pages | ~$1 | ~$0.1 |
| Medium | 50,000 | 250,000 pages | ~$10 | ~$1 |

- No AWS infrastructure cost (all local)
- Only cost is Gemini API usage

---

## 7. User Experience

### 7.1 Initial Setup (First Launch)

앱 첫 실행 시 사용자에게 다음을 안내:

1. **Gemini API Key 입력** — 앱이 동작하려면 Google Gemini API 키가 필요
   - 설정 화면에서 API 키 입력란 제공
   - API 키 발급 방법 안내 링크 포함 (https://aistudio.google.com/apikey)
   - 입력 후 유효성 검증 (Gemini API health check 호출)
   - 유효하지 않은 키 → 에러 메시지와 재입력 유도
   - API 키는 OS keychain에 저장 (macOS Keychain, Windows Credential Manager, Linux Secret Service)
2. **검색 대상 디렉토리 선택** — 두 가지 모드 제공:
   - **디렉토리 선택 모드** (기본) — 사용자가 지정한 폴더의 모든 하위 파일을 재귀적으로 분석
   - **전체 드라이브 스캔 모드** — 시스템의 모든 드라이브/마운트 포인트를 탐색하여 지원되는 모든 파일을 분석. OS/시스템 디렉토리(Windows: `C:\Windows`, macOS: `/System`, Linux: `/proc`, `/sys` 등), 숨김 폴더(`.git`, `node_modules` 등)는 기본 제외. 파일 수가 매우 많을 수 있으므로 예상 소요 시간/비용 안내 후 사용자 확인 필요.
3. **인덱싱 시작** — 선택한 범위의 파일을 백그라운드에서 인덱싱

API 키가 설정되지 않으면 검색/인덱싱 기능이 비활성화되어야 함.

### 7.2 Desktop GUI (Tauri)

- Search bar (main screen) — type natural language query
- Results list with file path, summary, similarity score
- Click to open file in default application
- Settings page: API key 관리, directory selection, indexing status
- System tray: background file watching daemon

### 7.2 Supported File Formats

| Format | Extensions | Processing Method |
|--------|-----------|-------------------|
| PDF | `.pdf` | Gemini Files API → direct analysis |
| Word | `.docx` | MarkItDown → Markdown → Gemini |
| PowerPoint | `.pptx` | MarkItDown → Markdown → Gemini |
| Excel | `.xlsx` | MarkItDown → Markdown → Gemini |
| Text | `.txt`, `.md`, `.csv` | Read directly → Gemini |
| Images | `.jpg`, `.png` | Gemini Files API → vision analysis |

---

## 8. Project Structure

```
semantic-file-search/
├── src-tauri/                       # Rust backend (Tauri)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                  # Tauri entry point
│       ├── crawler.rs               # File crawling / watching
│       ├── gemini.rs                # Gemini API client
│       ├── search.rs                # Vector search
│       ├── db.rs                    # SQLite management
│       └── config.rs                # Configuration management
│
├── src/                             # Frontend (React/Svelte)
│   ├── App.tsx
│   ├── components/
│   │   ├── SearchBar.tsx
│   │   ├── ResultList.tsx
│   │   └── Settings.tsx
│   └── ...
│
├── sidecar/                         # Python sidecar (MarkItDown)
│   ├── convert.py                   # MarkItDown conversion script
│   ├── requirements.txt             # markitdown
│   └── build.sh                     # PyInstaller build script
│
├── package.json                     # Frontend dependencies
└── tauri.conf.json                  # Tauri configuration
```

---

## 9. Milestones

### Phase 1: Foundation (MVP)
- [ ] Tauri project setup (Rust backend + frontend scaffold)
- [ ] MarkItDown sidecar: PyInstaller build + Tauri sidecar integration
- [ ] Gemini API client in Rust (summary + keywords + embedding)
- [ ] SQLite index DB implementation
- [ ] File crawling + indexing pipeline (end-to-end)
- [ ] Basic search UI (search bar + result list)
- [ ] Settings UI (API key, directory selection)

### Phase 2: Production-Ready
- [ ] File change watching (`notify`-based background daemon)
- [ ] Incremental indexing (process only changed files)
- [ ] Search quality tuning (prompt optimization)
- [ ] Rate limit handling + retry logic
- [ ] Error handling / logging improvements
- [ ] System tray integration
- [ ] Cross-platform installer build (Windows NSIS, macOS DMG, Linux AppImage)

### Phase 3: Extensions
- [ ] Search filters (date, file type, directory)
- [ ] Search history / favorites
- [ ] Multi-language UI
- [ ] HWP/HWPX support (별도 변환 필요 — MVP에서는 제외)

---

## 10. Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Gemini API outage | Indexing blocked | Retry queue + local cache of pending files |
| MarkItDown conversion failure | Specific files missing from index | Log failed files + skip gracefully |
| Large file processing latency | Poor UX | Progress indicator + background processing |
| Gemini model ID change | API calls fail | Manage model ID via config |
| Gemini Files API 48-hour expiry | File deleted before analysis | Process immediately after upload |
| Scanned document OCR accuracy | Reduced search quality | Use Gemini vision `media_resolution: high` option |
| Python sidecar bundling issues | App fails on specific OS | Test PyInstaller builds on all 3 platforms in CI |
| API key security | Key exposed in config file | OS keychain integration (macOS Keychain, Windows Credential Manager) |

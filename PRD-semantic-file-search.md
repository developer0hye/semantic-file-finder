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
- **Fully self-contained single-PC execution** — index DB, settings, embedding cache, and all data are stored on the user's local machine. The only external network communication is Gemini API calls
- **No server/cloud infrastructure required** — no dependency on external servers, DB servers, vector DB services, or any other external infrastructure
- Cross-platform desktop app (macOS, Ubuntu Linux, Windows) via Tauri

---

## 2. Architecture

### 2.1 Single Client Architecture

All components run on a single user's local PC. The only external dependency is Gemini API calls (HTTPS); all other processing and storage is performed locally.

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
                           │ HTTPS (Gemini API only — the sole external communication)
                           ▼
                   ┌───────────────┐
                   │ Google Gemini  │
                   │ API            │
                   └───────────────┘
```

### 2.2 Cross-Platform Design (macOS / Ubuntu / Windows)

To provide a consistent user experience across all 3 OS platforms, OS-specific differences are handled in an abstraction layer.

#### 2.2.1 Data Directory (App Data Storage Path)

All local data (DB, config, logs) is stored at the OS-standard path returned by Tauri's `app_data_dir()`. No hardcoded paths like `~/.semantic-file-search/`.

| OS | Data Directory | Example |
|----|---------------|---------|
| macOS | `~/Library/Application Support/semantic-file-search/` | Tauri `app_data_dir()` |
| Ubuntu | `~/.local/share/semantic-file-search/` | Follows XDG_DATA_HOME |
| Windows | `C:\Users\{user}\AppData\Roaming\semantic-file-search\` | %APPDATA% |

```
{app_data_dir}/
├── index.db              # SQLite (metadata + embedding + pending_files)
├── tantivy_index/        # Tantivy full-text search index (segments)
├── config.json           # User settings (excluding API key)
└── logs/                 # tracing log files
    └── app.log
```

#### 2.2.2 File Path Handling

| Item | Implementation |
|------|---------------|
| Path representation | Use `std::path::PathBuf` (automatic OS-specific separator handling) |
| DB storage | Always normalize `file_path` to **Unix-style (`/`)** for index portability |
| UI display | Convert to OS-native path when passing to frontend (`path.display()`) |
| Case sensitivity | macOS/Windows are case-insensitive, Ubuntu is case-sensitive — branch on OS for path comparison |
| Path length | Handle Windows MAX_PATH (260 char) limit: auto-prepend long path prefix (`\\?\`) |

```rust
// Cross-platform path handling in Rust
use std::path::PathBuf;
use tauri::api::path::app_data_dir;

fn normalize_path(path: &std::path::Path) -> String {
    // Normalize to Unix-style for DB storage
    path.to_string_lossy().replace('\\', "/")
}
```

#### 2.2.3 Default Exclude Directories

OS-specific system directories to exclude by default in full-drive scan mode:

| OS | Excluded Directories |
|----|---------------------|
| macOS | `/System`, `/Library`, `/private`, `/Volumes` (external drives), `~/Library`, `.Trash` |
| Ubuntu | `/proc`, `/sys`, `/dev`, `/run`, `/snap`, `/boot`, `/lost+found` |
| Windows | `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`, `C:\ProgramData`, `$Recycle.Bin` |
| Common | `.git`, `node_modules`, `.cache`, `__pycache__`, `.Trash*`, `Thumbs.db`, `.DS_Store` |

#### 2.2.4 File Watcher (notify crate) OS-Specific Backends

| OS | Backend | Notes |
|----|---------|-------|
| macOS | FSEvents | Stable, directory-level monitoring |
| Ubuntu | inotify | **Watch count limit** — default 8,192. Display guidance to increase `fs.inotify.max_user_watches` when monitoring many directories |
| Windows | ReadDirectoryChangesW | Stable, limited when watching network drives |

When inotify watch limit is exceeded on Ubuntu, display the following message to the user:
```
The number of monitored directories exceeds the system limit.
Please run the following command and restart the app:
echo fs.inotify.max_user_watches=524288 | sudo tee -a /etc/sysctl.conf && sudo sysctl -p
```

#### 2.2.5 System Tray / Menu Bar

| OS | Behavior |
|----|----------|
| macOS | Menu bar icon (top right) — app stays in background when window is closed |
| Ubuntu | AppIndicator (requires libappindicator3) — visibility varies by desktop environment |
| Windows | System tray icon (bottom right) — app stays in background when window is closed |

#### 2.2.6 Open File in Default App

Uses Tauri's `tauri::api::shell::open()`, which internally invokes OS-specific commands:

| OS | Internal Call |
|----|--------------|
| macOS | `open {path}` |
| Ubuntu | `xdg-open {path}` |
| Windows | `start "" "{path}"` |

#### 2.2.7 Webview Runtime

| OS | Webview | Installation Requirement |
|----|---------|------------------------|
| macOS | WebKit (WKWebView) | Built into the system — no additional installation |
| Ubuntu | WebKitGTK | `libwebkit2gtk-4.1-dev` package required (provide installation instructions) |
| Windows | WebView2 (Edge) | Included by default in Windows 10/11. Auto-installs on first launch if missing (Tauri's `webview_install_mode: downloadBootstrapper`) |

#### 2.2.8 OS Keychain Fallback

| OS | Primary Backend | Fallback |
|----|----------------|----------|
| macOS | Keychain Services | — (always available) |
| Ubuntu | Secret Service (GNOME Keyring) | If Secret Service is not installed → local encrypted file (`{app_data_dir}/credentials.enc`) + user warning |
| Windows | Credential Manager | — (always available) |

On Ubuntu server/minimal installations where GNOME Keyring may not be available, implement an AES-256 encrypted file fallback when the `keyring` crate fails.

### 2.3 Component Responsibilities

| Component | Role |
|-----------|------|
| File Crawler | Recursive directory traversal, change detection (`notify` crate) |
| anytomd | Convert DOCX/PPTX/XLSX/CSV → Markdown text + embedded image extraction (pure Rust crate, `cargo add anytomd`) |
| Gemini API Client | Send text/PDF/images to Gemini → get summary, keywords, embeddings |
| Local Index DB | Store file metadata + summaries + embedding vectors in SQLite |
| Tantivy Index | Full-text search index — keyword/phrase search on summary, keywords, and file name |
| Vector Search Engine | Cosine similarity between query embedding and document embeddings |
| Hybrid Ranker | Weighted sum of Tantivy score + Vector score to determine final ranking |
| Tauri Frontend | Desktop GUI (HTML/CSS/JS) for search and settings |

### 2.4 Document Processing by Format

| Input Format | Processing Pipeline |
|-------------|-------------------|
| PDF | Upload to Gemini Files API → analyze directly |
| DOCX | anytomd → Markdown text + image extraction → Gemini generateContent |
| PPTX | anytomd → Markdown text + image extraction → Gemini generateContent |
| XLSX | anytomd → Markdown text → Gemini generateContent (auto-extracts images if present) |
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
| Full-Text Search | `tantivy` | Rust-native full-text search engine (Lucene-class), keyword/phrase search |
| Vector Search | Custom implementation (cosine similarity) | Semantic search, brute-force for <100K files |
| Async Runtime | `tokio` | Parallel processing, file watching |
| Serialization | `serde` + `serde_json` | API communication |
| File Hashing | `blake3` | Fast content-based change detection |
| Error Handling | `thiserror` | Structured error types with derive macro |
| Logging | `tracing` + `tracing-subscriber` | Structured async-aware logging |
| Keychain | `keyring` | Cross-platform OS credential storage (macOS Keychain, Windows Credential Manager, Ubuntu Secret Service) |
| Platform Dirs | `dirs` | OS-standard directory path detection (Documents, Desktop, Home, etc.) |

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

**Decision: Use anytomd (Remove Python Sidecar)**

- anytomd is a pure Rust reimplementation of MarkItDown that converts tables/lists/formatting/images into LLM-optimized Markdown
- No Python sidecar needed, eliminating PyInstaller bundling, cross-platform build issues, and IPC overhead
- Built-in image extraction for DOCX/PPTX — no separate ZIP parsing code required
- Developed by the same author, enabling direct addition/modification of features needed for semantic-file-search

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
              (DOCX/PPTX: send markdown text + ConversionResult.images together)
5. Embed:     Summary text → Gemini Embedding API → 1536-dim vector
6. FT-Index:  Register document in Tantivy (summary + keywords + file_name → full-text index)
7. Store:     Save to local SQLite index (metadata + embedding)
```

### 4.2 Hybrid Search Flow

Search executes **keyword search (Tantivy)** and **semantic search (Vector)** simultaneously and combines the scores.

```
1. User input:    "Q3 revenue report 2024"
                         │
              ┌──────────┴──────────┐
              ▼                     ▼
2a. Tantivy Search             2b. Vector Search
    Query text as-is               Query → Gemini Embedding API
    full-text search               cosine similarity
    → keyword_score (BM25)         → vector_score (0.0~1.0)
              │                     │
              └──────────┬──────────┘
                         ▼
3. Hybrid Ranking:
    final_score = α × normalize(keyword_score) + (1-α) × vector_score
    α = 0.4 (default, user-configurable)
                         │
                         ▼
4. Display:       Top N results (file path + summary + score + match type)
```

**Behavior by search mode:**

| Query Type | Example | Tantivy Contribution | Vector Contribution |
|-----------|---------|---------------------|-------------------|
| Exact keyword | "John Smith contract 2024-03" | High (BM25 matching) | Low |
| Semantic | "last quarter performance report" | Low | High (semantic similarity) |
| Mixed | "Q3 revenue Excel" | Medium | Medium |

**Offline search support:**
- When Gemini API is unavailable (network disconnected), return results using Tantivy keyword search only (degraded mode)
- Display "Using keyword search only" notice to the user

### 4.3 anytomd Document Conversion + Image Extraction

anytomd is a Rust crate compiled directly into the Tauri binary. No Python sidecar, IPC, or separate process needed.

When processing DOCX/PPTX:
1. Call `anytomd::convert_file(path)` or `anytomd::convert_bytes(data, ext)`
2. Internally performs OOXML ZIP parsing → Markdown conversion + image binary extraction simultaneously
3. Result: `ConversionResult { markdown, images, title, warnings }`
4. `images: Vec<ImageData>` — each image's filename + binary data
5. When sending to Gemini, include both markdown text + images

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
- Image extraction internally handles OOXML Relationship ID (rId) → media file path mapping
- CSV, JSON, and Plain Text can also be processed through `convert_file()`

### 4.4 Indexing Concurrency Model

The indexing pipeline is implemented with tokio-based async processing, with the following concurrency controls.

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

| Stage | Concurrency Control | Implementation |
|-------|-------------------|----------------|
| File Queue | `tokio::sync::mpsc` unbounded channel | Crawler/file watcher enqueues discovered file paths |
| Convert (anytomd) | `tokio::task::spawn_blocking` × 4 | anytomd conversion is CPU-bound (ZIP/XML parsing) — runs in blocking task pool |
| Gemini API Call | `tokio::sync::Semaphore` (Free: 5, Paid: 50 permits) | Limit concurrent requests to match rate limit tier |
| Retry | Exponential backoff (1s → 2s → 4s → ... max 60s) | Auto-retry on 429/5xx responses, max 5 attempts |
| DB Write | `tokio::sync::RwLock` on DB connection | RwLock enables concurrent indexing (write) and search (read) |
| Search Requests | Search available during indexing (read lock) | Search responds immediately based on current DB state |

**Indexing pause/resume:**
- On app shutdown, save current queue state to SQLite `pending_files` table
- On app restart, resume processing from pending files
- Provide UI for users to manually pause/resume indexing

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

The Tantivy index is stored separately from SQLite in the `{app_data_dir}/tantivy_index/` directory.

```rust
// Tantivy schema definition
let mut schema_builder = Schema::builder();

// Searchable fields (tokenized, indexed)
schema_builder.add_text_field("summary", TEXT | STORED);     // Gemini-generated summary
schema_builder.add_text_field("keywords", TEXT | STORED);    // Gemini-extracted keywords
schema_builder.add_text_field("file_name", TEXT | STORED);   // File name (with extension)
schema_builder.add_text_field("file_path", STRING | STORED); // Full path (not tokenized)

// Filter/sort fields
schema_builder.add_text_field("file_ext", STRING | STORED);  // Extension filter
schema_builder.add_date_field("modified_at", INDEXED | STORED); // Date range filter
schema_builder.add_u64_field("file_size", INDEXED | STORED);    // Size filter

// SQLite link
schema_builder.add_u64_field("db_id", STORED);               // Reference to SQLite files.id
```

**Tokenizer configuration:**

| Language | Tokenizer | Notes |
|----------|-----------|-------|
| English | Tantivy default (`SimpleTokenizer` + `LowerCaser` + `Stemmer`) | Built-in |
| Korean | `lindera` tokenizer (MeCab dictionary-based) | Alternative: `tantivy-jieba`, uses `lindera-tantivy` crate |
| CJK Common | `lindera` + UniDic/IPAdic | Morphological analysis for Korean/Japanese/Chinese |

```rust
// Multi-language tokenizer for mixed Korean + English text
// Uses lindera's Korean dictionary (ko-dic) for morphological analysis
let tokenizer = LinderaTokenizer::with_config(LinderaConfig {
    mode: Mode::Normal,
    dict: DictType::KoDic,  // Korean dictionary
});
index.tokenizers().register("korean", tokenizer);
```

**Index storage structure:**
```
{app_data_dir}/
├── index.db                # SQLite (metadata + embedding)
├── tantivy_index/          # Tantivy full-text search index
│   ├── meta.json
│   └── segments/
└── ...
```

### 5.3 Configuration

```json
// {app_data_dir}/config.json (see Section 2.2.1)
// API key stored in OS keychain (see Section 7.4)
{
  "watch_directories": [],
  "supported_extensions": [".pdf", ".docx", ".pptx", ".xlsx", ".txt", ".md", ".csv", ".json", ".jpg", ".png"],
  "embedding_dimensions": 1536,
  "gemini_model": "gemini-3-flash-preview",
  "search_alpha": 0.4
}
```

**`watch_directories` default values per OS:**
- macOS: `["/Users/{user}/Documents", "/Users/{user}/Desktop"]`
- Ubuntu: `["/home/{user}/Documents", "/home/{user}/Desktop"]`
- Windows: `["C:\\Users\\{user}\\Documents", "C:\\Users\\{user}\\Desktop"]`

On first launch, the app auto-detects OS-standard paths using the Rust `dirs` crate (`dirs::document_dir()`, `dirs::desktop_dir()`, etc.) and suggests them as defaults.

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

### 6.3 PDF Processing via Gemini Files API (Official)

**PDF files MUST be processed using Gemini's native PDF understanding** — not converted to text or images first. Gemini natively analyzes PDF text, images, diagrams, charts, and tables in a single pass, preserving visual context that text extraction alone would lose.

Reference: https://ai.google.dev/gemini-api/docs/document-processing

#### Why Gemini-native PDF processing

| Approach | Quality | What's Lost |
|----------|---------|-------------|
| Gemini Files API (native) | Highest | Nothing — full visual + text context preserved |
| Text extraction → Gemini | Low | Tables, charts, images, layout, scanned text |
| Convert to images → Gemini | Medium | Searchable text, link structure |

#### Processing flow

```
1. Upload:   POST https://generativelanguage.googleapis.com/upload/v1beta/files
             Content-Type: application/pdf
             → returns { file: { uri: "...", state: "ACTIVE" } }

2. Analyze:  POST .../models/gemini-3-flash-preview:generateContent
             parts: [{ file_data: { file_uri: "...", mime_type: "application/pdf" } }]
             → returns summary, keywords, doc_type

3. Embed:    Send analysis summary to embedContent API
```

#### Technical limits

| Property | Value |
|----------|-------|
| Max file size | 50 MB |
| Max pages | 1,000 |
| Token cost per page | ~258 tokens (medium resolution) |
| Max resolution | 3,072 × 3,072 pixels per page |
| Scanned PDF (OCR) | Natively supported via Gemini vision |
| File expiration | 48 hours after upload — process immediately |

#### Implementation rules

- **Always upload PDF as-is** to the Files API. Never use anytomd or other converters for PDF.
- Use `application/pdf` as the MIME type.
- After upload, verify `state == "ACTIVE"` before sending to `generateContent`.
- Process (analyze + embed) immediately after upload to avoid 48-hour file expiration.
- For scanned PDFs or image-heavy PDFs, Gemini automatically applies OCR — no special handling needed.

### 6.4 Rate Limit Handling

| Tier | RPM | TPM | RPD |
|------|-----|-----|-----|
| Free | ~10 | 250,000 | ~1,000 |
| Tier 1 (Paid) | ~300 | 1,000,000 | ~1,000 |
| Tier 2 | ~1,000 | 2,000,000 | ~10,000 |

- Use **Batch API** for bulk indexing (50% discount, async processing)
- Implement exponential backoff retry logic
- Files API uploads have no separate rate limit

### 6.5 Cost Estimation (Single PC)

Estimated Gemini API costs when a single PC user indexes local files:

| File Count | Total Pages (avg 5p/file) | Initial Indexing (one-time) | Monthly Maintenance (changes only) |
|-----------|------------------------|--------------------------|------------------------------------|
| 500 | 2,500 pages | ~$0.1 | ~$0.01 |
| 5,000 | 25,000 pages | ~$1 | ~$0.1 |
| 50,000 | 250,000 pages | ~$10 | ~$1 |

- All processing runs on the user's local PC — no server/cloud infrastructure costs
- Costs are solely proportional to Gemini API usage
- 50% discount available when using Batch API

---

## 7. Rust Implementation Design

### 7.1 Error Type Hierarchy

Structured error types defined using `thiserror`. All errors ultimately unify into `AppError` for delivery to the frontend.

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    // Gemini API related
    #[error("Gemini API error: {message} (status: {status_code})")]
    GeminiApi { status_code: u16, message: String },

    #[error("Gemini rate limit exceeded, retry after {retry_after_secs}s")]
    GeminiRateLimit { retry_after_secs: u64 },

    #[error("Invalid API key")]
    InvalidApiKey,

    // File processing related
    #[error("File I/O error: {path}")]
    FileIo { path: String, #[source] source: std::io::Error },

    #[error("Unsupported file format: {extension}")]
    UnsupportedFormat { extension: String },

    // Document conversion related (anytomd)
    #[error("Document conversion failed: {path}")]
    ConversionFailed { path: String, #[source] source: anytomd::ConvertError },

    // DB related
    #[error("Database error")]
    Database(#[from] rusqlite::Error),

    // Search related
    #[error("Embedding generation failed for query")]
    EmbeddingFailed { query: String },
}
```

**Frontend delivery format:**

Serialized into a deliverable structure when passing to the frontend via Tauri IPC.

```rust
#[derive(serde::Serialize)]
pub struct ErrorResponse {
    pub code: String,          // "GEMINI_RATE_LIMIT", "FILE_IO", etc.
    pub message: String,       // User-facing message
    pub recoverable: bool,     // Whether auto-retry is possible
}

impl From<AppError> for ErrorResponse { ... }
```

### 7.2 Tauri IPC Command Definitions

Frontend ↔ Rust backend communication is defined via `#[tauri::command]`.

```rust
// === Search ===
#[tauri::command]
async fn search_files(
    query: String,
    limit: Option<usize>,
    mode: Option<SearchMode>,       // Hybrid (default), KeywordOnly, VectorOnly
    alpha: Option<f32>,             // Keyword weight (0.0~1.0, default 0.4)
) -> Result<SearchResponse, ErrorResponse>;

#[derive(serde::Deserialize)]
enum SearchMode {
    Hybrid,       // Tantivy + Vector (default)
    KeywordOnly,  // Tantivy only (auto-switch when offline)
    VectorOnly,   // Vector only
}

#[derive(serde::Serialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
    mode_used: SearchMode,          // Actual mode used (falls back to KeywordOnly when offline)
    query_time_ms: u64,             // Search duration
}

#[derive(serde::Serialize)]
struct SearchResult {
    file_path: String,
    file_name: String,
    file_ext: String,
    summary: String,
    keywords: Vec<String>,
    final_score: f32,               // Final hybrid score (0.0 ~ 1.0)
    keyword_score: Option<f32>,     // Tantivy BM25 score (normalized)
    vector_score: Option<f32>,      // Cosine similarity score
    modified_at: String,            // ISO 8601
}

// === Indexing ===
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
    current_file: Option<String>,   // Currently processing file path
}

// === Settings ===
#[tauri::command]
async fn validate_api_key(key: String) -> Result<bool, ErrorResponse>;

#[tauri::command]
async fn save_api_key(key: String) -> Result<(), ErrorResponse>;  // Save to OS keychain

#[tauri::command]
async fn get_config() -> Result<AppConfig, ErrorResponse>;

#[tauri::command]
async fn update_config(config: AppConfig) -> Result<(), ErrorResponse>;

// === File ===
#[tauri::command]
async fn open_file(file_path: String) -> Result<(), ErrorResponse>;  // Open with default app

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

Tantivy is a Rust-native full-text search engine and a key reason for using Rust in this app.

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
    // Returns (BM25 score, db_id)
    top_docs.into_iter().map(|(score, doc_addr)| {
        let doc = searcher.doc(doc_addr).unwrap();
        let db_id = doc.get_first(db_id_field).unwrap().as_u64().unwrap();
        (score, db_id)
    }).collect()
}
```

**Why Tantivy is better than Python alternatives:**

| Item | Tantivy (Rust) | Whoosh (Python) | SQLite FTS5 |
|------|---------------|-----------------|-------------|
| Search speed (100K docs) | ~1ms | ~50ms | ~10ms |
| Indexing speed | Very fast | Slow | Medium |
| Korean tokenization | lindera (MeCab) | None (custom implementation needed) | None (custom implementation needed) |
| Memory usage | mmap-based, low memory | Full load | Tied to DB |
| BM25 ranking | Built-in | Built-in | Limited |
| Real-time incremental | Supported | Supported | Supported |

#### 7.3.2 Vector Search

```rust
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (norm_a * norm_b)
}
```

**Vector search optimization path:**

| File Count | Strategy | Implementation |
|-----------|----------|---------------|
| < 10K | Brute-force (single thread) | `Vec<f32>` iteration |
| 10K ~ 50K | Brute-force + parallelization | `rayon::par_iter()` |
| 50K ~ 100K | SIMD + parallelization | f32 SIMD acceleration |
| > 100K (Phase 3) | ANN index | `usearch` crate |

#### 7.3.3 Hybrid Score Fusion

Score normalization + weighted summation logic for combining two search results:

```rust
/// Hybrid search: combine Tantivy (keyword) + Vector (semantic) results
fn hybrid_search(
    query: &str,
    query_embedding: &[f32],
    alpha: f32,  // Keyword weight (0.0~1.0)
    limit: usize,
) -> Vec<SearchResult> {
    // 1. Tantivy search (BM25 score)
    let keyword_results = keyword_search(query, limit * 2);

    // 2. Vector search (cosine similarity)
    let vector_results = vector_search(query_embedding, limit * 2);

    // 3. Normalize BM25 scores to 0.0~1.0 (min-max normalization)
    let keyword_normalized = normalize_scores(&keyword_results);

    // 4. Score fusion: final = α × keyword + (1-α) × vector
    let merged = merge_and_rank(keyword_normalized, vector_results, alpha);

    merged.into_iter().take(limit).collect()
}
```

**Alpha weight guide:**

| α Value | Keyword Weight | Semantic Weight | Suitable Query |
|---------|---------------|-----------------|---------------|
| 0.0 | 0% | 100% | "last quarter performance report" |
| 0.4 (default) | 40% | 60% | General mixed queries |
| 0.7 | 70% | 30% | "John Smith contract 2024-03" |
| 1.0 | 100% | 0% | Exact keyword search (offline mode) |

### 7.4 OS Keychain Integration

API keys are stored in the OS keychain for security instead of config files. (See Section 2.2.8 for OS-specific fallback policies)

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
    // Primary: try OS keychain
    match Entry::new(SERVICE_NAME, KEY_NAME)?.set_password(key) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Fallback (Ubuntu): local encrypted file
            store_api_key_encrypted(key)
        }
    }
}
```

---

## 8. User Experience

### 8.1 Initial Setup (First Launch)

On first launch, guide the user through:

1. **Gemini API Key Input** — the app requires a Google Gemini API key to function
   - Provide API key input field in the settings screen
   - Include link to API key issuance instructions (https://aistudio.google.com/apikey)
   - Validate key after input (Gemini API health check call)
   - Invalid key → error message and prompt to re-enter
   - API key stored in OS keychain (macOS Keychain, Windows Credential Manager, Ubuntu Secret Service)
2. **Search Target Directory Selection** — two modes available:
   - **Directory Selection Mode** (default) — recursively analyze all files in user-specified folders
   - **Full Drive Scan Mode** — explore all drives/mount points on the system to analyze all supported files. OS-specific system directories excluded by default (see Section 2.2.3). May have many files, so display estimated time/cost and require user confirmation.
3. **Start Indexing** — index files in the selected scope in the background

Search/indexing features must be disabled if no API key is configured.

### 8.2 Desktop GUI (Tauri)

- Search bar (main screen) — type natural language query
- Results list with file path, summary, similarity score
- Click to open file in default application
- Settings page: API key management, directory selection, indexing status
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
- [ ] Cross-platform base module (`platform.rs`) — path normalization, data directory, default exclude lists
- [ ] Rust error types (`AppError`) + logging (`tracing`) foundation
- [ ] OS keychain integration (`keyring` crate) — API key storage/retrieval + Ubuntu fallback
- [ ] anytomd integration (`converter.rs`) — `cargo add anytomd`, convert_file/convert_bytes wrapper
- [ ] Gemini API client in Rust (summary + keywords + embedding)
- [ ] SQLite index DB implementation (stored at `app_data_dir()` path)
- [ ] Tantivy full-text search index setup (schema + Korean tokenizer configuration)
- [ ] Indexing pipeline implementation (channel + semaphore-based concurrency, includes Tantivy indexing)
- [ ] File crawling + indexing pipeline (end-to-end)
- [ ] Hybrid search implementation (Tantivy BM25 + Vector cosine similarity + score fusion)
- [ ] Offline search fallback (Tantivy keyword-only mode)
- [ ] Register Tauri IPC commands (per Section 7.2 definitions)
- [ ] Basic search UI (search bar + result list + search mode selection)
- [ ] Settings UI (API key, directory selection, OS default path auto-detection)

### Phase 2: Production-Ready
- [ ] File change watching (`notify`-based background daemon)
- [ ] Incremental indexing (process only changed files)
- [ ] Search quality tuning (prompt optimization + alpha weight tuning)
- [ ] Rate limit handling + retry logic (exponential backoff)
- [ ] `rayon`-based vector search parallelization (for 10K+ files)
- [ ] Error handling / logging improvements
- [ ] System tray / menu bar integration (OS-specific behavior — Section 2.2.5)
- [ ] Ubuntu inotify watch limit exceeded notification UI
- [ ] Cross-platform installer build:
  - macOS: DMG (Universal Binary — Apple Silicon + Intel)
  - Ubuntu: AppImage + `.deb` (includes libwebkit2gtk dependency)
  - Windows: NSIS installer (includes WebView2 bootstrapper)

### Phase 3: Extensions
- [ ] Search filters (date, file type, directory) — leveraging Tantivy's date/string fields
- [ ] Search history / favorites
- [ ] Multi-language UI
- [ ] SIMD-optimized vector search or ANN index introduction (for 100K+ files)
- [ ] Expand supported formats when anytomd adds HTML/PDF conversion
- [ ] HWP/HWPX support (requires separate conversion — excluded from MVP)

---

## 11. Risks and Mitigations

### 11.1 General Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| Gemini API outage | Indexing blocked | Retry queue + save to `pending_files` table for resume on restart |
| anytomd conversion failure | Specific files missing from index | anytomd performs best-effort conversion — partial failures reported via `ConversionResult.warnings`. On complete failure, log + skip gracefully |
| Large file processing latency | Poor UX | Progress indicator + background processing + indexing pause/resume UI |
| Gemini model ID change | API calls fail | Manage model ID via config |
| Gemini Files API 48-hour expiry | File deleted before analysis | Process immediately after upload |
| Scanned document OCR accuracy | Reduced search quality | Use Gemini vision `media_resolution: high` option |
| API key security | Key exposed in config file | Store in OS keychain (`keyring` crate), exclude key from config.json |
| anytomd unsupported formats | Cannot convert PDF, HWP, etc. | Send PDF/images directly via Gemini Files API, skip unsupported formats + notify user |
| Large embedding memory usage | ~600MB memory at 100K files | Switch to mmap-based access or ANN index in Phase 3 |
| App shutdown during indexing | Work loss | Persist queue state in `pending_files` table, auto-resume on restart |

### 11.2 Cross-Platform Risks

| Risk | Affected OS | Impact | Mitigation |
|------|------------|--------|------------|
| Ubuntu inotify watch limit | Ubuntu | Large directory monitoring fails | Detect error and display `sysctl` configuration change instructions in UI |
| GNOME Keyring not installed on Ubuntu | Ubuntu | API key storage fails | Local encrypted file fallback + user warning (Section 2.2.8) |
| WebKitGTK not installed on Ubuntu | Ubuntu | App cannot launch | Declare dependency in `.deb` package + provide installation documentation |
| Windows MAX_PATH limit | Windows | Files with long paths fail to index | Auto-prepend long path prefix (`\\?\`), set `longPathAware` in manifest |
| macOS code signing not applied | macOS | Gatekeeper blocks app execution | Code sign with Apple Developer ID + notarize (Phase 2) |
| Windows Defender false positive | Windows | App blocked as malware | Use EV code signing certificate (Phase 2) |
| File path case sensitivity differences | Ubuntu | Conflicts with same-name files in different case | Preserve original path in DB, branch on OS detection for comparison |

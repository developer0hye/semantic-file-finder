// TypeScript interfaces matching Rust IPC types from commands.rs

export type SearchMode = "Hybrid" | "KeywordOnly" | "VectorOnly";

export type IndexingState = "Idle" | "Running" | "Paused";

export interface SearchResultItem {
  file_path: string;
  file_name: string;
  summary: string;
  keywords: string;
  final_score: number;
  keyword_score: number;
  vector_score: number;
}

export interface SearchResponse {
  results: SearchResultItem[];
  mode_used: string;
  query_time_ms: number;
}

export interface IndexingStatus {
  state: IndexingState;
  total_files: number;
  indexed_files: number;
  failed_files: number;
  current_file: string | null;
}

export interface IndexedStats {
  total_files: number;
  by_extension: Record<string, number>;
  total_size_bytes: number;
}

export interface AppConfig {
  watch_directories: string[];
  supported_extensions: string[];
  embedding_model: string;
  embedding_dimensions: number;
  gemini_model: string;
  search_alpha: number;
}

export interface WatcherStatus {
  is_running: boolean;
  watched_directories: string[];
}

export interface SearchFilters {
  file_types?: string[];
  date_after?: number;
  date_before?: number;
  directories?: string[];
}

export interface ErrorResponse {
  code: string;
  message: string;
  recoverable: boolean;
}

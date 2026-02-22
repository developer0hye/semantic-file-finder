import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  IndexedStats,
  IndexingStatus,
  SearchMode,
  SearchResponse,
} from "./types";

// === Search ===

export async function searchFiles(
  query: string,
  limit?: number,
  mode?: SearchMode,
  alpha?: number,
): Promise<SearchResponse> {
  return invoke("search_files", { query, limit, mode, alpha });
}

// === Indexing ===

export async function startIndexing(directories: string[]): Promise<void> {
  return invoke("start_indexing", { directories });
}

export async function pauseIndexing(): Promise<void> {
  return invoke("pause_indexing");
}

export async function resumeIndexing(): Promise<void> {
  return invoke("resume_indexing");
}

export async function getIndexingStatus(): Promise<IndexingStatus> {
  return invoke("get_indexing_status");
}

// === Settings ===

export async function validateApiKey(key: string): Promise<boolean> {
  return invoke("validate_api_key", { key });
}

export async function saveApiKey(key: string): Promise<void> {
  return invoke("save_api_key", { key });
}

export async function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

export async function updateConfig(newConfig: AppConfig): Promise<void> {
  return invoke("update_config", { newConfig });
}

// === Extensions ===

export async function getAllSupportedExtensions(): Promise<string[]> {
  return invoke("get_all_supported_extensions");
}

// === File ===

export async function openFile(filePath: string): Promise<void> {
  return invoke("open_file", { filePath });
}

export async function getIndexedStats(): Promise<IndexedStats> {
  return invoke("get_indexed_stats");
}

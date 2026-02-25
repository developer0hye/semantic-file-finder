import { useState } from "react";
import type { SearchFilters, SearchMode } from "../types";

interface SearchBarProps {
  onSearch: (query: string, mode: SearchMode, alpha: number, filters?: SearchFilters) => void;
  isSearching: boolean;
}

const MODES: { value: SearchMode; label: string }[] = [
  { value: "Hybrid", label: "Hybrid" },
  { value: "KeywordOnly", label: "Keyword" },
  { value: "VectorOnly", label: "Vector" },
];

const COMMON_FILE_TYPES = [
  { ext: "pdf", label: "PDF" },
  { ext: "docx", label: "DOCX" },
  { ext: "txt", label: "TXT" },
  { ext: "xlsx", label: "XLSX" },
  { ext: "pptx", label: "PPTX" },
  { ext: "md", label: "MD" },
  { ext: "csv", label: "CSV" },
  { ext: "png", label: "PNG" },
  { ext: "jpg", label: "JPG" },
];

export default function SearchBar({ onSearch, isSearching }: SearchBarProps) {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<SearchMode>("Hybrid");
  const [alpha, setAlpha] = useState(0.4);
  const [showFilters, setShowFilters] = useState(false);
  const [selectedTypes, setSelectedTypes] = useState<string[]>([]);
  const [dateAfter, setDateAfter] = useState("");
  const [dateBefore, setDateBefore] = useState("");
  const [directoryInput, setDirectoryInput] = useState("");

  function buildFilters(): SearchFilters | undefined {
    const hasTypeFilter = selectedTypes.length > 0;
    const hasDateFilter = dateAfter !== "" || dateBefore !== "";
    const hasDirFilter = directoryInput.trim() !== "";

    if (!hasTypeFilter && !hasDateFilter && !hasDirFilter) return undefined;

    const filters: SearchFilters = {};
    if (hasTypeFilter) filters.file_types = selectedTypes;
    if (dateAfter) filters.date_after = Math.floor(new Date(dateAfter).getTime() / 1000);
    if (dateBefore) {
      // Set to end of day for the "before" date
      const d = new Date(dateBefore);
      d.setHours(23, 59, 59);
      filters.date_before = Math.floor(d.getTime() / 1000);
    }
    if (hasDirFilter) {
      filters.directories = directoryInput
        .split(",")
        .map((d) => d.trim())
        .filter((d) => d.length > 0);
    }
    return filters;
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = query.trim();
    const filters = buildFilters();
    // Allow empty query if filters are active (browse mode)
    if (trimmed.length === 0 && !filters) return;
    onSearch(trimmed, mode, alpha, filters);
  }

  function toggleFileType(ext: string) {
    setSelectedTypes((prev) =>
      prev.includes(ext) ? prev.filter((t) => t !== ext) : [...prev, ext],
    );
  }

  function clearFilters() {
    setSelectedTypes([]);
    setDateAfter("");
    setDateBefore("");
    setDirectoryInput("");
  }

  const hasActiveFilters =
    selectedTypes.length > 0 || dateAfter !== "" || dateBefore !== "" || directoryInput.trim() !== "";
  const canSubmit = query.trim().length > 0 || hasActiveFilters;

  return (
    <form className="search-bar" onSubmit={handleSubmit}>
      <div className="search-input-row">
        <input
          type="text"
          className="search-input"
          placeholder="Search files with natural language..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          disabled={isSearching}
        />
        <button type="submit" className="search-button" disabled={isSearching || !canSubmit}>
          {isSearching ? "Searching..." : "Search"}
        </button>
      </div>
      <div className="search-options">
        <div className="mode-selector">
          {MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              className={`mode-button ${mode === m.value ? "active" : ""}`}
              onClick={() => setMode(m.value)}
            >
              {m.label}
            </button>
          ))}
          <button
            type="button"
            className={`mode-button ${showFilters ? "active" : ""} ${hasActiveFilters ? "filter-active" : ""}`}
            onClick={() => setShowFilters(!showFilters)}
          >
            Filters{hasActiveFilters ? " *" : ""}
          </button>
        </div>
        {mode === "Hybrid" && (
          <label className="alpha-slider">
            <span>Keyword weight: {alpha.toFixed(2)}</span>
            <input
              type="range"
              min="0"
              max="1"
              step="0.05"
              value={alpha}
              onChange={(e) => setAlpha(parseFloat(e.target.value))}
            />
          </label>
        )}
      </div>
      {showFilters && (
        <div className="search-filters">
          <div className="filter-section">
            <label className="filter-label">File type</label>
            <div className="file-type-chips">
              {COMMON_FILE_TYPES.map((ft) => (
                <button
                  key={ft.ext}
                  type="button"
                  className={`chip ${selectedTypes.includes(ft.ext) ? "active" : ""}`}
                  onClick={() => toggleFileType(ft.ext)}
                >
                  {ft.label}
                </button>
              ))}
            </div>
          </div>
          <div className="filter-section">
            <label className="filter-label">Date range</label>
            <div className="date-range">
              <input
                type="date"
                value={dateAfter}
                onChange={(e) => setDateAfter(e.target.value)}
                placeholder="From"
              />
              <span className="date-separator">to</span>
              <input
                type="date"
                value={dateBefore}
                onChange={(e) => setDateBefore(e.target.value)}
                placeholder="To"
              />
            </div>
          </div>
          <div className="filter-section">
            <label className="filter-label">Directory</label>
            <input
              type="text"
              className="filter-input"
              placeholder="/Users/me/Documents (comma-separated)"
              value={directoryInput}
              onChange={(e) => setDirectoryInput(e.target.value)}
            />
          </div>
          {hasActiveFilters && (
            <button type="button" className="clear-filters-button" onClick={clearFilters}>
              Clear filters
            </button>
          )}
        </div>
      )}
    </form>
  );
}

import { useState } from "react";
import type { SearchMode } from "../types";

interface SearchBarProps {
  onSearch: (query: string, mode: SearchMode, alpha: number) => void;
  isSearching: boolean;
}

const MODES: { value: SearchMode; label: string }[] = [
  { value: "Hybrid", label: "Hybrid" },
  { value: "KeywordOnly", label: "Keyword" },
  { value: "VectorOnly", label: "Vector" },
];

export default function SearchBar({ onSearch, isSearching }: SearchBarProps) {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<SearchMode>("Hybrid");
  const [alpha, setAlpha] = useState(0.4);

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = query.trim();
    if (trimmed.length === 0) return;
    onSearch(trimmed, mode, alpha);
  }

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
        <button type="submit" className="search-button" disabled={isSearching || query.trim().length === 0}>
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
    </form>
  );
}

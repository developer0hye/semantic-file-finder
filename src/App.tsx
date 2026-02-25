import { useState } from "react";
import type { SearchFilters, SearchMode, SearchResponse } from "./types";
import { searchFiles } from "./api";
import SearchBar from "./components/SearchBar";
import ResultList from "./components/ResultList";
import Settings from "./components/Settings";

type View = "search" | "settings";

function App() {
  const [view, setView] = useState<View>("search");
  const [isSearching, setIsSearching] = useState(false);
  const [searchResponse, setSearchResponse] = useState<SearchResponse | null>(null);
  const [searchError, setSearchError] = useState<string | null>(null);

  async function handleSearch(query: string, mode: SearchMode, alpha: number, filters?: SearchFilters) {
    setIsSearching(true);
    setSearchError(null);
    try {
      const response = await searchFiles(query, 20, mode, alpha, filters);
      setSearchResponse(response);
    } catch (err) {
      setSearchError(String(err));
      setSearchResponse(null);
    } finally {
      setIsSearching(false);
    }
  }

  return (
    <div className="app">
      <nav className="app-nav">
        <button
          className={`nav-button ${view === "search" ? "active" : ""}`}
          onClick={() => setView("search")}
        >
          Search
        </button>
        <button
          className={`nav-button ${view === "settings" ? "active" : ""}`}
          onClick={() => setView("settings")}
        >
          Settings
        </button>
      </nav>

      <main className="app-content">
        {view === "search" && (
          <>
            <SearchBar onSearch={handleSearch} isSearching={isSearching} />
            {searchError && <div className="error-banner">{searchError}</div>}
            <ResultList response={searchResponse} />
          </>
        )}
        {view === "settings" && <Settings />}
      </main>
    </div>
  );
}

export default App;

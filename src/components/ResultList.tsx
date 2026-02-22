import type { SearchResponse, SearchResultItem } from "../types";
import { openFile } from "../api";

interface ResultListProps {
  response: SearchResponse | null;
}

function formatScore(score: number): string {
  return (score * 100).toFixed(1);
}

function ResultCard({ item }: { item: SearchResultItem }) {
  async function handleClick() {
    try {
      await openFile(item.file_path);
    } catch (err) {
      console.error("Failed to open file:", err);
    }
  }

  return (
    <div className="result-card" onClick={handleClick} role="button" tabIndex={0}
      onKeyDown={(e) => { if (e.key === "Enter") handleClick(); }}>
      <div className="result-header">
        <span className="result-filename">{item.file_name}</span>
        <span className="result-score">{formatScore(item.final_score)}%</span>
      </div>
      <p className="result-path">{item.file_path}</p>
      <p className="result-summary">{item.summary}</p>
      <div className="result-scores">
        <span className="score-badge keyword">K: {formatScore(item.keyword_score)}%</span>
        <span className="score-badge vector">V: {formatScore(item.vector_score)}%</span>
      </div>
    </div>
  );
}

export default function ResultList({ response }: ResultListProps) {
  if (!response) return null;

  const { results, mode_used, query_time_ms } = response;

  return (
    <div className="result-list">
      <div className="result-meta">
        <span>{results.length} result{results.length !== 1 ? "s" : ""}</span>
        <span>{query_time_ms}ms</span>
        <span className="mode-badge">{mode_used}</span>
      </div>
      {results.length === 0 ? (
        <p className="no-results">No results found.</p>
      ) : (
        results.map((item) => <ResultCard key={item.file_path} item={item} />)
      )}
    </div>
  );
}

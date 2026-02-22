import { useEffect, useState } from "react";
import type { AppConfig, IndexedStats, IndexingStatus } from "../types";
import {
  getConfig,
  getIndexedStats,
  getIndexingStatus,
  pauseIndexing,
  resumeIndexing,
  saveApiKey,
  startIndexing,
  updateConfig,
  validateApiKey,
} from "../api";

export default function Settings() {
  const [apiKey, setApiKey] = useState("");
  const [apiKeyStatus, setApiKeyStatus] = useState<"idle" | "validating" | "valid" | "invalid">("idle");

  const [config, setConfig] = useState<AppConfig | null>(null);
  const [newDirectory, setNewDirectory] = useState("");

  const [indexingStatus, setIndexingStatus] = useState<IndexingStatus | null>(null);
  const [stats, setStats] = useState<IndexedStats | null>(null);

  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadSettings();
    const interval = setInterval(pollIndexingStatus, 2000);
    return () => clearInterval(interval);
  }, []);

  async function loadSettings() {
    try {
      const [cfg, st, status] = await Promise.all([
        getConfig(),
        getIndexedStats(),
        getIndexingStatus(),
      ]);
      setConfig(cfg);
      setStats(st);
      setIndexingStatus(status);
    } catch (err) {
      setError(String(err));
    }
  }

  async function pollIndexingStatus() {
    try {
      const status = await getIndexingStatus();
      setIndexingStatus(status);
    } catch {
      // Silently ignore polling errors
    }
  }

  async function handleSaveApiKey() {
    if (apiKey.trim().length === 0) return;
    setApiKeyStatus("validating");
    setError(null);
    try {
      const valid = await validateApiKey(apiKey.trim());
      if (valid) {
        await saveApiKey(apiKey.trim());
        setApiKeyStatus("valid");
        setApiKey("");
      } else {
        setApiKeyStatus("invalid");
      }
    } catch (err) {
      setApiKeyStatus("invalid");
      setError(String(err));
    }
  }

  async function handleAddDirectory() {
    if (!config || newDirectory.trim().length === 0) return;
    const dir = newDirectory.trim();
    if (config.watch_directories.includes(dir)) return;
    const updated = { ...config, watch_directories: [...config.watch_directories, dir] };
    try {
      await updateConfig(updated);
      setConfig(updated);
      setNewDirectory("");
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleRemoveDirectory(dir: string) {
    if (!config) return;
    const updated = {
      ...config,
      watch_directories: config.watch_directories.filter((d) => d !== dir),
    };
    try {
      await updateConfig(updated);
      setConfig(updated);
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleStartIndexing() {
    if (!config || config.watch_directories.length === 0) return;
    setError(null);
    try {
      await startIndexing(config.watch_directories);
    } catch (err) {
      setError(String(err));
    }
  }

  async function handlePauseResume() {
    if (!indexingStatus) return;
    try {
      if (indexingStatus.state === "Running") {
        await pauseIndexing();
      } else if (indexingStatus.state === "Paused") {
        await resumeIndexing();
      }
    } catch (err) {
      setError(String(err));
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
  }

  return (
    <div className="settings">
      {error && <div className="error-banner">{error}</div>}

      <section className="settings-section">
        <h2>API Key</h2>
        <p className="settings-hint">
          Get your key from{" "}
          <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener noreferrer">
            Google AI Studio
          </a>
        </p>
        <div className="api-key-row">
          <input
            type="password"
            className="api-key-input"
            placeholder="Enter Gemini API key..."
            value={apiKey}
            onChange={(e) => {
              setApiKey(e.target.value);
              setApiKeyStatus("idle");
            }}
          />
          <button
            onClick={handleSaveApiKey}
            disabled={apiKey.trim().length === 0 || apiKeyStatus === "validating"}
          >
            {apiKeyStatus === "validating" ? "Validating..." : "Save"}
          </button>
        </div>
        {apiKeyStatus === "valid" && <p className="status-ok">API key saved successfully.</p>}
        {apiKeyStatus === "invalid" && <p className="status-error">Invalid API key. Please check and try again.</p>}
      </section>

      <section className="settings-section">
        <h2>Directories</h2>
        <div className="directory-add-row">
          <input
            type="text"
            className="directory-input"
            placeholder="Enter directory path..."
            value={newDirectory}
            onChange={(e) => setNewDirectory(e.target.value)}
            onKeyDown={(e) => { if (e.key === "Enter") handleAddDirectory(); }}
          />
          <button onClick={handleAddDirectory} disabled={newDirectory.trim().length === 0}>
            Add
          </button>
        </div>
        <ul className="directory-list">
          {config?.watch_directories.map((dir) => (
            <li key={dir}>
              <span className="directory-path">{dir}</span>
              <button className="remove-button" onClick={() => handleRemoveDirectory(dir)}>
                Remove
              </button>
            </li>
          ))}
          {config?.watch_directories.length === 0 && (
            <li className="no-dirs">No directories configured.</li>
          )}
        </ul>
      </section>

      <section className="settings-section">
        <h2>Indexing</h2>
        {indexingStatus && (
          <div className="indexing-info">
            <div className="indexing-state">
              <span className={`state-indicator ${indexingStatus.state.toLowerCase()}`}>
                {indexingStatus.state}
              </span>
              {indexingStatus.state !== "Idle" && (
                <span>
                  {indexingStatus.indexed_files} / {indexingStatus.total_files} files
                  {indexingStatus.failed_files > 0 && (
                    <span className="failed-count"> ({indexingStatus.failed_files} failed)</span>
                  )}
                </span>
              )}
            </div>
            {indexingStatus.state !== "Idle" && indexingStatus.total_files > 0 && (
              <div className="progress-bar">
                <div
                  className="progress-fill"
                  style={{
                    width: `${(indexingStatus.indexed_files / indexingStatus.total_files) * 100}%`,
                  }}
                />
              </div>
            )}
            {indexingStatus.current_file && (
              <p className="current-file">Processing: {indexingStatus.current_file}</p>
            )}
          </div>
        )}
        <div className="indexing-controls">
          <button onClick={handleStartIndexing} disabled={indexingStatus?.state === "Running"}>
            Start Indexing
          </button>
          {(indexingStatus?.state === "Running" || indexingStatus?.state === "Paused") && (
            <button onClick={handlePauseResume}>
              {indexingStatus.state === "Running" ? "Pause" : "Resume"}
            </button>
          )}
        </div>
      </section>

      {stats && (
        <section className="settings-section">
          <h2>Index Statistics</h2>
          <div className="stats-grid">
            <div className="stat-item">
              <span className="stat-value">{stats.total_files}</span>
              <span className="stat-label">Total Files</span>
            </div>
            <div className="stat-item">
              <span className="stat-value">{formatBytes(stats.total_size_bytes)}</span>
              <span className="stat-label">Total Size</span>
            </div>
          </div>
          {Object.keys(stats.by_extension).length > 0 && (
            <div className="extension-breakdown">
              <h3>By Extension</h3>
              <ul>
                {Object.entries(stats.by_extension)
                  .sort(([, a], [, b]) => b - a)
                  .map(([ext, count]) => (
                    <li key={ext}>
                      <span>{ext}</span>
                      <span>{count}</span>
                    </li>
                  ))}
              </ul>
            </div>
          )}
        </section>
      )}
    </div>
  );
}

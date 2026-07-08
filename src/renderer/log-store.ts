// In-memory real-time activity log shared between the logging calculator API
// wrapper and the LogConsole UI. Kept renderer-only and deliberately simple: an
// append-only ring buffer with an external-store subscription so the console can
// use `useSyncExternalStore` without tearing.

export type LogLevel = "info" | "success" | "warn" | "error";

export interface LogEntry {
  id: number;
  timestamp: string; // ISO 8601, UTC
  level: LogLevel;
  scope: string; // e.g. "action", "native", "ble", "env"
  message: string;
  detail?: string; // optional multi-line detail (pretty JSON, state summary)
}

const MAX_ENTRIES = 1000;

type Listener = () => void;

class LogStore {
  private entries: LogEntry[] = [];
  private listeners = new Set<Listener>();
  private nextId = 1;

  subscribe = (listener: Listener): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  // Returns a stable reference until the next mutation so React can bail out of
  // re-renders when nothing changed.
  getSnapshot = (): LogEntry[] => this.entries;

  push(level: LogLevel, scope: string, message: string, detail?: string): void {
    const entry: LogEntry = {
      id: this.nextId++,
      timestamp: new Date().toISOString(),
      level,
      scope,
      message,
      detail
    };

    const next = this.entries.concat(entry);
    this.entries =
      next.length > MAX_ENTRIES ? next.slice(next.length - MAX_ENTRIES) : next;
    this.emit();
  }

  clear(): void {
    if (this.entries.length === 0) {
      return;
    }
    this.entries = [];
    this.emit();
  }

  // Plain-text export for pasting into a bug report / debugging session.
  toText(): string {
    return this.entries.map(formatEntryForCopy).join("\n");
  }

  private emit(): void {
    for (const listener of this.listeners) {
      listener();
    }
  }
}

export function formatEntryForCopy(entry: LogEntry): string {
  const head = `[${entry.timestamp}] ${entry.level.toUpperCase().padEnd(7)} ${entry.scope.padEnd(8)} ${entry.message}`;
  if (!entry.detail) {
    return head;
  }
  const indented = entry.detail
    .split("\n")
    .map((line) => `    ${line}`)
    .join("\n");
  return `${head}\n${indented}`;
}

export const logStore = new LogStore();

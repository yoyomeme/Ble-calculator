import React, { useCallback, useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import {
  Bluetooth,
  Calculator,
  Check,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  Delete,
  PlugZap,
  Radio,
  ShieldCheck,
  ShieldQuestion,
  Users,
  Wifi
} from "lucide-react";
import type { PeerSummary, RoomState, SessionRole } from "../shared/calculator-api";
import { calculateExpression } from "../shared/expression";
import { createBrowserCalculatorApi } from "./browser-calculator";
import "./styles.css";

const emptyState: RoomState = {
  localDeviceId: "loading",
  roomId: null,
  roomName: null,
  sessionRole: null,
  bleRole: null,
  scanning: false,
  advertising: false,
  peers: [],
  history: []
};

type CalculatorKey =
  | {
      label: string;
      value: string;
      role?: "number" | "operator";
      ariaLabel?: string;
    }
  | {
      label: string;
      action: "clear" | "delete" | "equals";
      role: "danger" | "utility" | "equals";
      ariaLabel?: string;
    };

const calculatorKeys: CalculatorKey[] = [
  { label: "AC", action: "clear", role: "danger", ariaLabel: "Clear expression" },
  { label: "DEL", action: "delete", role: "utility", ariaLabel: "Delete last character" },
  { label: "%", value: "%", role: "operator", ariaLabel: "Modulo" },
  { label: "÷", value: "/", role: "operator", ariaLabel: "Divide" },
  { label: "7", value: "7" },
  { label: "8", value: "8" },
  { label: "9", value: "9" },
  { label: "×", value: "*", role: "operator", ariaLabel: "Multiply" },
  { label: "4", value: "4" },
  { label: "5", value: "5" },
  { label: "6", value: "6" },
  { label: "-", value: "-", role: "operator", ariaLabel: "Subtract" },
  { label: "1", value: "1" },
  { label: "2", value: "2" },
  { label: "3", value: "3" },
  { label: "+", value: "+", role: "operator", ariaLabel: "Add" },
  { label: "0", value: "0" },
  { label: ".", value: "." },
  { label: "=", action: "equals", role: "equals", ariaLabel: "Calculate and sync" }
];

const BOTH_DRAWERS_MIN_VIEWPORT_WIDTH = 1126;

function canFitBothDrawers(): boolean {
  return window.innerWidth >= BOTH_DRAWERS_MIN_VIEWPORT_WIDTH;
}

function App() {
  const calculatorApi = useMemo(() => window.calculator ?? createBrowserCalculatorApi(), []);
  const [state, setState] = useState<RoomState>(emptyState);
  const [roomName, setRoomName] = useState("Desk Calculator");
  const [roomCode, setRoomCode] = useState("DESK-01");
  const [expression, setExpression] = useState("7 + 5 * 2");
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hostOpen, setHostOpen] = useState(canFitBothDrawers);
  const [historyOpen, setHistoryOpen] = useState(canFitBothDrawers);
  const [canOpenBothDrawers, setCanOpenBothDrawers] = useState(canFitBothDrawers);
  const [lastOpenedDrawer, setLastOpenedDrawer] = useState<"host" | "history" | null>(null);

  const runAction = useCallback(
    async (label: string, action: () => Promise<RoomState>) => {
      setPendingAction(label);
      setError(null);
      try {
        setState(await action());
      } catch (caught) {
        setError(caught instanceof Error ? caught.message : "Unexpected native bridge error");
      } finally {
        setPendingAction(null);
      }
    },
    []
  );

  useEffect(() => {
    void runAction("Loading", () => calculatorApi.getState());
  }, [calculatorApi, runAction]);

  useEffect(() => {
    const updateDrawerCapacity = () => {
      setCanOpenBothDrawers(canFitBothDrawers());
    };

    updateDrawerCapacity();
    window.addEventListener("resize", updateDrawerCapacity);

    return () => {
      window.removeEventListener("resize", updateDrawerCapacity);
    };
  }, []);

  useEffect(() => {
    if (canOpenBothDrawers || !hostOpen || !historyOpen) {
      return;
    }

    if (lastOpenedDrawer === "host") {
      setHistoryOpen(false);
    } else {
      setHostOpen(false);
    }
  }, [canOpenBothDrawers, historyOpen, hostOpen, lastOpenedDrawer]);

  const connectedPeers = useMemo(
    () => state.peers.filter((peer) => peer.connected).length,
    [state.peers]
  );

  const currentRole = state.sessionRole ?? "host";
  const previewResult = useMemo(() => calculateExpression(expression), [expression]);
  const hasValidPreview = previewResult !== "Invalid expression";

  const handleKeyPress = useCallback(
    (key: CalculatorKey) => {
      if ("action" in key) {
        if (key.action === "clear") {
          setExpression("");
          return;
        }

        if (key.action === "delete") {
          setExpression((current) => current.slice(0, -1));
          return;
        }

        void runAction("Syncing result", () =>
          calculatorApi.submitCalculation({ expression })
        );
        return;
      }

      setExpression((current) => `${current}${key.value}`);
    },
    [calculatorApi, expression, runAction]
  );

  const toggleHostBench = useCallback(() => {
    setHostOpen((open) => {
      const nextOpen = !open;
      if (nextOpen) {
        setLastOpenedDrawer("host");
        if (!canOpenBothDrawers) {
          setHistoryOpen(false);
        }
      }
      return nextOpen;
    });
  }, [canOpenBothDrawers]);

  const toggleHistory = useCallback(() => {
    setHistoryOpen((open) => {
      const nextOpen = !open;
      if (nextOpen) {
        setLastOpenedDrawer("history");
        if (!canOpenBothDrawers) {
          setHostOpen(false);
        }
      }
      return nextOpen;
    });
  }, [canOpenBothDrawers]);

  return (
    <main className="app-shell">
      <div className="app-backdrop" aria-hidden="true" />

      <section
        className={`workspace ${hostOpen ? "host-open" : ""} ${historyOpen ? "history-open" : ""}`}
      >
        <aside
          className={`left-panel drawer drawer--left ${hostOpen ? "is-open" : ""}`}
          aria-label="Session setup"
        >
          <button
            type="button"
            className="drawer-toggle drawer-toggle--left"
            onClick={toggleHostBench}
            aria-label={hostOpen ? "Collapse host bench" : "Expand host bench"}
            aria-expanded={hostOpen}
          >
            <ChevronRight size={18} aria-hidden="true" />
          </button>
          <PanelHeader eyebrow="Room" title="Host Bench" />
          <SegmentedRole value={currentRole} actualRole={state.sessionRole} />

          <div className="control-group">
            <label htmlFor="room-name">Host room</label>
            <div className="input-row">
              <input
                id="room-name"
                value={roomName}
                onChange={(event) => setRoomName(event.target.value)}
                spellCheck={false}
              />
              <button
                type="button"
                onClick={() =>
                  void runAction("Creating room", () => calculatorApi.createRoom({ roomName }))
                }
                disabled={pendingAction !== null}
              >
                <Radio size={17} aria-hidden="true" />
                Start
              </button>
            </div>
          </div>

          <div className="control-group">
            <label htmlFor="room-code">Guest advertising</label>
            <div className="input-row">
              <input
                id="room-code"
                value={roomCode}
                onChange={(event) => setRoomCode(event.target.value)}
                spellCheck={false}
              />
              <button
                type="button"
                onClick={() =>
                  void runAction("Advertising", () =>
                    calculatorApi.startAdvertising({ roomCode })
                  )
                }
                disabled={pendingAction !== null}
              >
                <Wifi size={17} aria-hidden="true" />
                Signal
              </button>
            </div>
          </div>

          <div className="button-grid">
            <button
              type="button"
              className={state.scanning ? "active" : ""}
              onClick={() =>
                void runAction("Scanning", () => calculatorApi.startScanning())
              }
              disabled={pendingAction !== null || state.sessionRole === "guest"}
              title="Scan for advertising guests"
            >
              <CircleDot size={18} aria-hidden="true" />
              Scan
            </button>
            <button
              type="button"
              className={state.advertising ? "active" : ""}
              onClick={() =>
                void runAction("Accepting host", () => calculatorApi.acceptHostConnection())
              }
              disabled={pendingAction !== null || state.sessionRole !== "guest"}
              title="Mock accepting the host connection"
            >
              <PlugZap size={18} aria-hidden="true" />
              Accept
            </button>
          </div>

          <SessionFacts state={state} pendingAction={pendingAction} />
          {error ? <div className="error-box">{error}</div> : null}
        </aside>

        <section className="calculator-panel" aria-label="Evolve calculator">
          <header className="calculator-header">
            <div className="calculator-brand">
              <span className="brand-dot">
                <Calculator size={18} aria-hidden="true" />
              </span>
              <span>Evolve Calc</span>
            </div>
            <span className="mode-chip">Standard</span>
          </header>

          <div className="display" aria-live="polite">
            <span className="display-label">Expression</span>
            <strong className="display-expression">{expression || "0"}</strong>
            <output className={hasValidPreview ? "display-result" : "display-result invalid"}>
              {hasValidPreview ? previewResult : "Waiting"}
            </output>
          </div>

          <div className="calc-entry">
            <input
              aria-label="Calculation expression"
              value={expression}
              onChange={(event) => setExpression(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  void runAction("Submitting calculation", () =>
                    calculatorApi.submitCalculation({ expression })
                  );
                }
              }}
              spellCheck={false}
            />
            <button
              className="sync-button"
              type="button"
              onClick={() =>
                void runAction("Syncing result", () =>
                  calculatorApi.submitCalculation({ expression })
                )
              }
              disabled={pendingAction !== null}
            >
              <Check size={18} aria-hidden="true" />
              Sync
            </button>
          </div>

          <div className="keypad" aria-label="Calculator keypad">
            {calculatorKeys.map((key) => {
              const role = "role" in key && key.role ? key.role : "number";

              return (
                <button
                  type="button"
                  key={key.label}
                  className={`key key--${role}`}
                  onClick={() => handleKeyPress(key)}
                  aria-label={key.ariaLabel ?? key.label}
                  disabled={pendingAction !== null && "action" in key && key.action === "equals"}
                >
                  {key.label === "DEL" ? <Delete size={18} aria-hidden="true" /> : key.label}
                </button>
              );
            })}
          </div>
        </section>

        <aside
          className={`right-panel drawer drawer--right ${historyOpen ? "is-open" : ""}`}
          aria-label="Peers and history"
        >
          <button
            type="button"
            className="drawer-toggle drawer-toggle--right"
            onClick={toggleHistory}
            aria-label={historyOpen ? "Collapse history" : "Expand history"}
            aria-expanded={historyOpen}
          >
            <ChevronLeft size={18} aria-hidden="true" />
          </button>
          <section className="panel-section">
            <div className="section-title">
              <div>
                <span>Network</span>
                <h2>Peers</h2>
              </div>
              <StatusBeacon status={state.scanning ? "Scanning" : state.advertising ? "Signal" : "Idle"} />
            </div>
            <div className="peer-list">
              {state.peers.length === 0 ? (
                <p className="muted">No peers discovered yet.</p>
              ) : (
                state.peers.map((peer) => (
                  <PeerRow
                    key={peer.id}
                    peer={peer}
                    disabled={pendingAction !== null}
                    onConnect={() =>
                      void runAction("Connecting guest", () =>
                        calculatorApi.connectGuest({ peerId: peer.id })
                      )
                    }
                  />
                ))
              )}
            </div>
          </section>

          <section className="panel-section history-section">
            <div className="section-title">
              <div>
                <span>Ledger</span>
                <h2>History</h2>
              </div>
              <span>{state.history.length} events</span>
            </div>
            <div className="history-list">
              {state.history.length === 0 ? (
                <p className="muted">Submitted calculations appear here.</p>
              ) : (
                state.history.map((entry) => (
                  <article className="history-item" key={entry.id}>
                    <div>
                      <strong>{entry.expression}</strong>
                      <span>{entry.originDeviceId}</span>
                    </div>
                    <output>{entry.result}</output>
                  </article>
                ))
              )}
            </div>
          </section>
        </aside>
      </section>

      <section className="status-rail" aria-label="Application status">
        <StatusPill icon={<Bluetooth size={16} />} label={state.bleRole ?? "no BLE role"} />
        <StatusPill
          icon={<Users size={16} />}
          label={`${connectedPeers}/${state.peers.length} connected`}
        />
        <StatusPill
          icon={<ShieldCheck size={16} />}
          label={connectedPeers > 0 ? "trusted session" : "trust pending"}
        />
      </section>
    </main>
  );
}

function PanelHeader({ eyebrow, title }: { eyebrow: string; title: string }) {
  return (
    <div className="panel-header">
      <span>{eyebrow}</span>
      <h2>{title}</h2>
    </div>
  );
}

function StatusPill({ icon, label }: { icon: React.ReactNode; label: string }) {
  return (
    <div className="status-pill">
      {icon}
      <span>{label}</span>
    </div>
  );
}

function SegmentedRole({
  value,
  actualRole
}: {
  value: SessionRole;
  actualRole: SessionRole | null;
}) {
  return (
    <div className="segmented" aria-label="Session role">
      <button type="button" className={value === "host" ? "selected" : ""}>
        Host
      </button>
      <button type="button" className={value === "guest" ? "selected" : ""}>
        Guest
      </button>
      <span>{actualRole ? "Active" : "Ready"}</span>
    </div>
  );
}

function SessionFacts({
  state,
  pendingAction
}: {
  state: RoomState;
  pendingAction: string | null;
}) {
  return (
    <dl className="facts">
      <div>
        <dt>Device</dt>
        <dd>{shortId(state.localDeviceId)}</dd>
      </div>
      <div>
        <dt>Room</dt>
        <dd>{state.roomId ?? "None"}</dd>
      </div>
      <div>
        <dt>Action</dt>
        <dd>{pendingAction ?? "Ready"}</dd>
      </div>
      <div>
        <dt>Trust</dt>
        <dd>{state.peers.some((peer) => peer.connected) ? "Trusted" : "Pending"}</dd>
      </div>
    </dl>
  );
}

function StatusBeacon({ status }: { status: string }) {
  return (
    <span className="status-beacon">
      <span />
      {status}
    </span>
  );
}

function PeerRow({
  peer,
  disabled,
  onConnect
}: {
  peer: PeerSummary;
  disabled: boolean;
  onConnect: () => void;
}) {
  return (
    <article className="peer-row">
      <div className="peer-icon">
        {peer.trustStatus === "trusted" ? (
          <ShieldCheck size={20} aria-hidden="true" />
        ) : (
          <ShieldQuestion size={20} aria-hidden="true" />
        )}
      </div>
      <div className="peer-main">
        <strong>{peer.label}</strong>
        <span>
          {peer.sessionRole} / {peer.bleRole}
        </span>
      </div>
      <button type="button" onClick={onConnect} disabled={disabled || peer.connected}>
        {peer.connected ? "Connected" : "Connect"}
      </button>
    </article>
  );
}

function shortId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 10)}...${id.slice(-4)}` : id;
}

const root = document.getElementById("root");

if (!root) {
  throw new Error("Renderer root element is missing");
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);

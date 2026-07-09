import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore
} from "react";
import { createRoot } from "react-dom/client";
import {
  Bluetooth,
  Calculator,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  ChevronUp,
  Copy,
  Delete,
  DoorOpen,
  Radio,
  Search,
  ShieldCheck,
  ShieldQuestion,
  Terminal,
  Trash2,
  Unplug,
  Users
} from "lucide-react";
import type {
  PeerSummary,
  RoomState,
  RoomSummary,
  SessionRole,
  TrustStatus
} from "../shared/calculator-api";
import { calculateExpression } from "../shared/expression";
import { createBrowserCalculatorApi } from "./browser-calculator";
import { logStore } from "./log-store";
import { createLoggingCalculatorApi, logStartupEnvironment } from "./logging-calculator-api";
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

type NetworkTab = "discovery" | "history";

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
  const calculatorApi = useMemo(
    () => createLoggingCalculatorApi(window.calculator ?? createBrowserCalculatorApi()),
    []
  );
  const [state, setState] = useState<RoomState>(emptyState);
  const [logOpen, setLogOpen] = useState(false);
  const [roomName, setRoomName] = useState("Desk Calculator");
  const [roomCode, setRoomCode] = useState("DESK-01");
  const [expression, setExpression] = useState("7 + 5 * 2");
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hostOpen, setHostOpen] = useState(canFitBothDrawers);
  const [historyOpen, setHistoryOpen] = useState(canFitBothDrawers);
  const [canOpenBothDrawers, setCanOpenBothDrawers] = useState(canFitBothDrawers);
  const [lastOpenedDrawer, setLastOpenedDrawer] = useState<"host" | "history" | null>(null);

  // Local UI role: which set of Room-panel fields to show. This is separate from
  // the backend `state.sessionRole`, which only becomes set once an action runs.
  const [uiRole, setUiRole] = useState<SessionRole>("host");
  const [networkTab, setNetworkTab] = useState<NetworkTab>("discovery");
  const [roleSwitchTarget, setRoleSwitchTarget] = useState<SessionRole | null>(null);

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
    logStartupEnvironment(Boolean(window.calculator));
    void runAction("Loading", () => calculatorApi.getState());
  }, [calculatorApi, runAction]);

  const toggleLog = useCallback(() => setLogOpen((open) => !open), []);

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

  const connectedPeer = useMemo(
    () => state.peers.find((peer) => peer.connected) ?? null,
    [state.peers]
  );
  const connectedPeers = useMemo(
    () => state.peers.filter((peer) => peer.connected).length,
    [state.peers]
  );
  const isConnected = connectedPeer !== null;
  // A session is "active" once anything BLE-related is in flight or established.
  const sessionActive =
    state.roomId !== null || state.advertising || state.scanning || isConnected;

  const previewResult = useMemo(() => calculateExpression(expression), [expression]);
  const hasValidPreview = previewResult !== "Invalid expression";

  const requestRoleSwitch = useCallback(
    (role: SessionRole) => {
      if (pendingAction !== null || uiRole === role) {
        return;
      }
      if (sessionActive) {
        setRoleSwitchTarget(role);
        return;
      }
      setUiRole(role);
    },
    [pendingAction, sessionActive, uiRole]
  );

  const confirmRoleSwitch = useCallback(async () => {
    const target = roleSwitchTarget;
    if (!target) {
      return;
    }
    await runAction("Ending session", () => calculatorApi.resetBleSession());
    setUiRole(target);
    setRoleSwitchTarget(null);
    setNetworkTab("discovery");
  }, [calculatorApi, roleSwitchTarget, runAction]);

  const cancelRoleSwitch = useCallback(() => setRoleSwitchTarget(null), []);

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

  const pending = pendingAction !== null;
  const roomCreated = state.roomId !== null;
  const networkStatus = state.scanning
    ? "Scanning"
    : state.advertising
      ? "Signal"
      : isConnected
        ? "Connected"
        : "Idle";

  return (
    <main className="app-shell">
      <div className="app-backdrop" aria-hidden="true" />

      <section
        className={`workspace ${hostOpen ? "host-open" : ""} ${historyOpen ? "history-open" : ""}`}
      >
        <aside
          className={`left-panel drawer drawer--left ${hostOpen ? "is-open" : ""}`}
          aria-label="Room setup"
        >
          <button
            type="button"
            className="drawer-toggle drawer-toggle--left"
            onClick={toggleHostBench}
            aria-label={hostOpen ? "Collapse room panel" : "Expand room panel"}
            aria-expanded={hostOpen}
          >
            <ChevronRight size={18} aria-hidden="true" />
          </button>
          <PanelHeader eyebrow="Room" title="Connection" />

          <RoleSwitcher value={uiRole} disabled={pending} onSelect={requestRoleSwitch} />

          {uiRole === "host" ? (
            <HostFields
              roomName={roomName}
              onRoomNameChange={setRoomName}
              roomCreated={roomCreated}
              scanning={state.scanning}
              isConnected={isConnected}
              pending={pending}
              onCreateRoom={() =>
                void runAction("Creating room", () => calculatorApi.createRoom({ roomName }))
              }
              onFindGuests={() =>
                void runAction("Finding guests", () => calculatorApi.startScanning())
              }
            />
          ) : (
            <GuestFields
              roomCode={roomCode}
              onRoomCodeChange={setRoomCode}
              scanning={state.scanning}
              isConnected={isConnected}
              pending={pending}
              onFindHosts={() =>
                void runAction("Finding hosts", () => calculatorApi.scanRooms())
              }
              onJoinByCode={() =>
                void runAction("Joining by code", () =>
                  calculatorApi.startAdvertising({ roomCode })
                )
              }
            />
          )}

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
              disabled={pending}
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
                  disabled={pending && "action" in key && key.action === "equals"}
                >
                  {key.label === "DEL" ? <Delete size={18} aria-hidden="true" /> : key.label}
                </button>
              );
            })}
          </div>
        </section>

        <aside
          className={`right-panel drawer drawer--right ${historyOpen ? "is-open" : ""}`}
          aria-label="Network and history"
        >
          <button
            type="button"
            className="drawer-toggle drawer-toggle--right"
            onClick={toggleHistory}
            aria-label={historyOpen ? "Collapse network panel" : "Expand network panel"}
            aria-expanded={historyOpen}
          >
            <ChevronLeft size={18} aria-hidden="true" />
          </button>
          <section className="panel-section network-section">
            <div className="section-title">
              <div>
                <span>Network</span>
                <h2>Session</h2>
              </div>
              <StatusBeacon status={networkStatus} />
            </div>

            <NetworkTabs value={networkTab} onChange={setNetworkTab} />

            {networkTab === "discovery" ? (
              <DiscoveryPane
                uiRole={uiRole}
                scanning={state.scanning}
                rooms={state.rooms ?? []}
                peers={state.peers}
                connectedPeer={connectedPeer}
                pending={pending}
                onConnectGuest={(peerId) =>
                  void runAction("Connecting guest", () =>
                    calculatorApi.connectGuest({ peerId })
                  )
                }
                onJoinRoom={(roomId) =>
                  void runAction("Joining room", () => calculatorApi.joinRoom({ roomId }))
                }
                onDisconnect={() =>
                  void runAction("Disconnecting", () => calculatorApi.resetBleSession())
                }
              />
            ) : (
              <HistoryPane history={state.history} />
            )}
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

      {roleSwitchTarget ? (
        <RoleSwitchModal
          target={roleSwitchTarget}
          pending={pending}
          onConfirm={() => void confirmRoleSwitch()}
          onCancel={cancelRoleSwitch}
        />
      ) : null}

      <LogConsole open={logOpen} onToggle={toggleLog} />
    </main>
  );
}

function LogConsole({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const entries = useSyncExternalStore(logStore.subscribe, logStore.getSnapshot);
  const bodyRef = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);

  const counts = useMemo(() => {
    let warn = 0;
    let error = 0;
    for (const entry of entries) {
      if (entry.level === "warn") {
        warn += 1;
      } else if (entry.level === "error") {
        error += 1;
      }
    }
    return { warn, error };
  }, [entries]);

  // Keep the newest lines in view while the console is open.
  useEffect(() => {
    if (open && bodyRef.current) {
      bodyRef.current.scrollTop = bodyRef.current.scrollHeight;
    }
  }, [entries, open]);

  const handleCopy = useCallback(async () => {
    const text = logStore.toText();
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Fallback for contexts where the async clipboard API is unavailable.
      const textarea = document.createElement("textarea");
      textarea.value = text;
      textarea.style.position = "fixed";
      textarea.style.opacity = "0";
      document.body.appendChild(textarea);
      textarea.select();
      try {
        document.execCommand("copy");
      } finally {
        document.body.removeChild(textarea);
      }
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  }, []);

  return (
    <section className={`log-console ${open ? "is-open" : ""}`} aria-label="Real-time activity log">
      <header className="log-console-bar">
        <button
          type="button"
          className="log-console-toggle"
          onClick={onToggle}
          aria-expanded={open}
          aria-controls="log-console-body"
        >
          <Terminal size={16} aria-hidden="true" />
          <span className="log-console-label">Activity Log</span>
          <span className="log-console-count">{entries.length}</span>
          {counts.error > 0 ? (
            <span className="log-badge log-badge--error">{counts.error}</span>
          ) : null}
          {counts.warn > 0 ? (
            <span className="log-badge log-badge--warn">{counts.warn}</span>
          ) : null}
          {open ? (
            <ChevronDown size={16} aria-hidden="true" className="log-console-chevron" />
          ) : (
            <ChevronUp size={16} aria-hidden="true" className="log-console-chevron" />
          )}
        </button>
        <div className="log-console-tools">
          <button type="button" onClick={() => void handleCopy()} disabled={entries.length === 0}>
            <Copy size={15} aria-hidden="true" />
            {copied ? "Copied" : "Copy"}
          </button>
          <button
            type="button"
            className="danger-outline"
            onClick={() => logStore.clear()}
            disabled={entries.length === 0}
          >
            <Trash2 size={15} aria-hidden="true" />
            Clear
          </button>
        </div>
      </header>

      {open ? (
        <div className="log-console-body" id="log-console-body" ref={bodyRef}>
          {entries.length === 0 ? (
            <p className="log-empty">No activity yet. Interact with the app to record logs.</p>
          ) : (
            entries.map((entry) => (
              <div key={entry.id} className={`log-line log-line--${entry.level}`}>
                <span className="log-time">{entry.timestamp.slice(11, 23)}</span>
                <span className="log-scope">{entry.scope}</span>
                <span className="log-message">{entry.message}</span>
                {entry.detail ? <pre className="log-detail">{entry.detail}</pre> : null}
              </div>
            ))
          )}
        </div>
      ) : null}
    </section>
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

function RoleSwitcher({
  value,
  disabled,
  onSelect
}: {
  value: SessionRole;
  disabled: boolean;
  onSelect: (role: SessionRole) => void;
}) {
  return (
    <div className="segmented" aria-label="Session role">
      <button
        type="button"
        className={value === "host" ? "selected" : ""}
        onClick={() => onSelect("host")}
        disabled={disabled}
        aria-pressed={value === "host"}
      >
        Host
      </button>
      <button
        type="button"
        className={value === "guest" ? "selected" : ""}
        onClick={() => onSelect("guest")}
        disabled={disabled}
        aria-pressed={value === "guest"}
      >
        Guest
      </button>
    </div>
  );
}

function HostFields({
  roomName,
  onRoomNameChange,
  roomCreated,
  scanning,
  isConnected,
  pending,
  onCreateRoom,
  onFindGuests
}: {
  roomName: string;
  onRoomNameChange: (value: string) => void;
  roomCreated: boolean;
  scanning: boolean;
  isConnected: boolean;
  pending: boolean;
  onCreateRoom: () => void;
  onFindGuests: () => void;
}) {
  return (
    <section className="role-fields" aria-label="Host controls">
      <div className="control-group">
        <label htmlFor="room-name">Room name</label>
        <input
          id="room-name"
          value={roomName}
          onChange={(event) => onRoomNameChange(event.target.value)}
          disabled={roomCreated}
          readOnly={roomCreated}
          spellCheck={false}
        />
      </div>
      <button
        type="button"
        className="primary-action"
        onClick={onCreateRoom}
        disabled={roomName.trim() === "" || pending || roomCreated}
      >
        <Radio size={17} aria-hidden="true" />
        {roomCreated ? "Room Created" : "Create Room"}
      </button>
      <button
        type="button"
        className="primary-action"
        onClick={onFindGuests}
        disabled={!roomCreated || isConnected || scanning || pending}
      >
        <Search size={17} aria-hidden="true" />
        {scanning ? "Scanning…" : "Find Guests"}
      </button>
    </section>
  );
}

function GuestFields({
  roomCode,
  onRoomCodeChange,
  scanning,
  isConnected,
  pending,
  onFindHosts,
  onJoinByCode
}: {
  roomCode: string;
  onRoomCodeChange: (value: string) => void;
  scanning: boolean;
  isConnected: boolean;
  pending: boolean;
  onFindHosts: () => void;
  onJoinByCode: () => void;
}) {
  return (
    <section className="role-fields" aria-label="Guest controls">
      <button
        type="button"
        className="primary-action"
        onClick={onFindHosts}
        disabled={isConnected || scanning || pending}
      >
        <Search size={17} aria-hidden="true" />
        {scanning ? "Searching…" : "Find Hosts"}
      </button>

      <div className="control-group control-group--secondary">
        <label htmlFor="room-code">Or join by code</label>
        <div className="input-row">
          <input
            id="room-code"
            value={roomCode}
            onChange={(event) => onRoomCodeChange(event.target.value)}
            spellCheck={false}
          />
          <button
            type="button"
            onClick={onJoinByCode}
            disabled={roomCode.trim() === "" || pending || isConnected}
          >
            <DoorOpen size={17} aria-hidden="true" />
            Join
          </button>
        </div>
      </div>
    </section>
  );
}

function NetworkTabs({
  value,
  onChange
}: {
  value: NetworkTab;
  onChange: (tab: NetworkTab) => void;
}) {
  return (
    <div className="network-tabs" role="tablist" aria-label="Network view">
      <button
        type="button"
        role="tab"
        aria-selected={value === "discovery"}
        className={value === "discovery" ? "selected" : ""}
        onClick={() => onChange("discovery")}
      >
        Discovery
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={value === "history"}
        className={value === "history" ? "selected" : ""}
        onClick={() => onChange("history")}
      >
        History
      </button>
    </div>
  );
}

function DiscoveryPane({
  uiRole,
  scanning,
  rooms,
  peers,
  connectedPeer,
  pending,
  onConnectGuest,
  onJoinRoom,
  onDisconnect
}: {
  uiRole: SessionRole;
  scanning: boolean;
  rooms: RoomSummary[];
  peers: PeerSummary[];
  connectedPeer: PeerSummary | null;
  pending: boolean;
  onConnectGuest: (peerId: string) => void;
  onJoinRoom: (roomId: string) => void;
  onDisconnect: () => void;
}) {
  if (connectedPeer) {
    return (
      <ConnectionCard peer={connectedPeer} pending={pending} onDisconnect={onDisconnect} />
    );
  }

  if (scanning && uiRole === "host") {
    const guests = peers.filter((peer) => peer.sessionRole === "guest" && !peer.connected);
    return (
      <div className="tab-content">
        <div className="discovery-list" aria-label="Nearby guests">
          {guests.length === 0 ? (
            <p className="muted">Searching for guests…</p>
          ) : (
            guests.map((guest) => (
              <DiscoveryRow
                key={guest.id}
                icon={<ShieldQuestion size={20} aria-hidden="true" />}
                title={guest.label}
                subtitle={`${guest.sessionRole} / ${guest.bleRole}`}
                rssi={guest.rssi}
                disabled={pending}
                onClick={() => onConnectGuest(guest.id)}
              />
            ))
          )}
        </div>
      </div>
    );
  }

  if (scanning && uiRole === "guest") {
    return (
      <div className="tab-content">
        <div className="discovery-list" aria-label="Available hosts">
          {rooms.length === 0 ? (
            <p className="muted">Searching for hosts…</p>
          ) : (
            rooms.map((room) => (
              <DiscoveryRow
                key={room.id}
                icon={<DoorOpen size={20} aria-hidden="true" />}
                title={room.name}
                subtitle={shortId(room.id)}
                rssi={room.rssi}
                disabled={pending || !room.joinable}
                onClick={() => onJoinRoom(room.id)}
              />
            ))
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="tab-content">
      <p className="muted discovery-empty">
        Not searching. Use Find Guests / Find Hosts in the Room panel to begin.
      </p>
    </div>
  );
}

function DiscoveryRow({
  icon,
  title,
  subtitle,
  rssi,
  disabled,
  onClick
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  rssi?: number | null;
  disabled: boolean;
  onClick: () => void;
}) {
  return (
    <button type="button" className="discovery-row" onClick={onClick} disabled={disabled}>
      <span className="peer-icon">{icon}</span>
      <span className="peer-main">
        <strong>{title}</strong>
        <span>{subtitle}</span>
      </span>
      <RssiIndicator rssi={rssi} />
    </button>
  );
}

function ConnectionCard({
  peer,
  pending,
  onDisconnect
}: {
  peer: PeerSummary;
  pending: boolean;
  onDisconnect: () => void;
}) {
  return (
    <div className="tab-content">
      <article className="connection-card" aria-label="Connected peer">
        <div className="connection-peer">
          <span className="peer-icon">
            {peer.trustStatus === "trusted" ? (
              <ShieldCheck size={20} aria-hidden="true" />
            ) : (
              <ShieldQuestion size={20} aria-hidden="true" />
            )}
          </span>
          <span className="peer-main">
            <strong>{peer.label}</strong>
            <span>
              {peer.sessionRole} / {peer.bleRole}
            </span>
          </span>
        </div>
        <div className="connection-meta">
          <TrustBadge status={peer.trustStatus} />
          <RssiIndicator rssi={peer.rssi} />
        </div>
        <button
          type="button"
          className="danger-outline connection-disconnect"
          onClick={onDisconnect}
          disabled={pending}
        >
          <Unplug size={17} aria-hidden="true" />
          Disconnect
        </button>
      </article>
    </div>
  );
}

function TrustBadge({ status }: { status: TrustStatus }) {
  return <span className={`trust-badge trust-badge--${status}`}>{status}</span>;
}

function RssiIndicator({ rssi }: { rssi?: number | null }) {
  // Native BLE does not report RSSI yet: render a placeholder, never a fake number.
  if (typeof rssi !== "number") {
    return (
      <span className="rssi rssi--empty" aria-label="Signal strength unavailable">
        —
      </span>
    );
  }

  const bars = rssiToBars(rssi);
  return (
    <span className="rssi" aria-label={`${rssi} dBm`}>
      <span className="rssi-value">{rssi}</span>
      <span className="rssi-bars" aria-hidden="true">
        {[1, 2, 3, 4].map((level) => (
          <span key={level} className={level <= bars ? "on" : ""} />
        ))}
      </span>
    </span>
  );
}

function rssiToBars(rssi: number): number {
  if (rssi >= -55) {
    return 4;
  }
  if (rssi >= -65) {
    return 3;
  }
  if (rssi >= -75) {
    return 2;
  }
  return 1;
}

function HistoryPane({ history }: { history: RoomState["history"] }) {
  return (
    <div className="tab-content">
      <div className="history-list">
        {history.length === 0 ? (
          <p className="muted">Submitted calculations appear here.</p>
        ) : (
          history.map((entry) => (
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
    </div>
  );
}

function RoleSwitchModal({
  target,
  pending,
  onConfirm,
  onCancel
}: {
  target: SessionRole;
  pending: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="modal-overlay" role="dialog" aria-modal="true" aria-label="Switch role">
      <div className="modal">
        <h3>Switch to {target === "host" ? "Host" : "Guest"}?</h3>
        <p>Switching roles will end your current room session. Continue?</p>
        <div className="modal-actions">
          <button
            type="button"
            className="primary-action"
            onClick={onConfirm}
            disabled={pending}
          >
            Continue
          </button>
          <button
            type="button"
            className="danger-outline"
            onClick={onCancel}
            disabled={pending}
          >
            Cancel
          </button>
        </div>
      </div>
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

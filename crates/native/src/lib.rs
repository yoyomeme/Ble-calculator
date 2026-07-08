use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use btleplug::api::{Central, Manager as _, Peripheral as _, PeripheralProperties, ScanFilter};
use btleplug::platform::Manager;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use once_cell::sync::Lazy;
use rand_core::OsRng;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

const DEFAULT_HISTORY_LIMIT: usize = 100;
const KEYCHAIN_SERVICE: &str = "io.evolve.ble-calculator";
const KEYCHAIN_USER: &str = "device-signing-key-v1";
const CALCULATOR_SERVICE_UUID: &str = "7c14f94a-77dd-4a65-9f04-6f7ac8d2a601";
const ADVERTISEMENT_PREFIX: &str = "EvolveCalc";
const JOIN_ADVERTISEMENT_KIND: &str = "JOIN";
const ROOM_ADVERTISEMENT_KIND: &str = "ROOM";
const BLE_CHUNK_PAYLOAD_SIZE: usize = 180;

static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerSummary {
    id: String,
    label: String,
    session_role: String,
    ble_role: String,
    trust_status: String,
    connected: bool,
    last_seen_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoomSummary {
    id: String,
    name: String,
    host_device_id: String,
    trust_status: String,
    joinable: bool,
    last_seen_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalculationEntry {
    id: String,
    origin_device_id: String,
    expression: String,
    result: String,
    trusted: bool,
    created_at_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeCapabilities {
    ble_central_scanning: bool,
    ble_peripheral_advertising: bool,
    sqlite_persistence: bool,
    keychain_storage: bool,
    local_jws_signing: bool,
    jwe_decryption: bool,
    jwt_sd_jwt_verification: bool,
    issuer_trust_validation: bool,
    holder_key_binding: bool,
    cross_device_sync: bool,
}

impl NativeCapabilities {
    fn current() -> Self {
        Self {
            ble_central_scanning: true,
            ble_peripheral_advertising: false,
            sqlite_persistence: true,
            keychain_storage: true,
            local_jws_signing: true,
            jwe_decryption: false,
            jwt_sd_jwt_verification: false,
            issuer_trust_validation: false,
            holder_key_binding: true,
            cross_device_sync: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeRuntimeStatus {
    sqlite_path: Option<String>,
    keychain_backed: bool,
    public_key_fingerprint: String,
    last_ble_error: Option<String>,
    last_validation: Option<ValidationSummary>,
    pending_outbox_events: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidationSummary {
    valid: bool,
    kind: String,
    issuer_trusted: bool,
    holder_bound: bool,
    reason: String,
}

impl ValidationSummary {
    fn local_event_valid(reason: impl Into<String>) -> Self {
        Self {
            valid: true,
            kind: "local-jws-calculation".to_string(),
            issuer_trusted: true,
            holder_bound: true,
            reason: reason.into(),
        }
    }

    fn unsupported(kind: &str, reason: impl Into<String>) -> Self {
        Self {
            valid: false,
            kind: kind.to_string(),
            issuer_trusted: false,
            holder_bound: false,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoomState {
    local_device_id: String,
    room_id: Option<String>,
    room_name: Option<String>,
    session_role: Option<String>,
    ble_role: Option<String>,
    scanning: bool,
    advertising: bool,
    peers: Vec<PeerSummary>,
    rooms: Vec<RoomSummary>,
    history: Vec<CalculationEntry>,
    native_capabilities: NativeCapabilities,
    native_status: NativeRuntimeStatus,
    native_warnings: Vec<String>,
}

impl RoomState {
    fn new() -> Self {
        let mut warnings = Vec::new();
        let identity = load_or_create_identity(&mut warnings);
        let storage = NativeStorage::open(&mut warnings);
        let history = storage
            .as_ref()
            .and_then(|store| store.load_history(DEFAULT_HISTORY_LIMIT, &mut warnings))
            .unwrap_or_default();
        let pending_outbox_events = storage
            .as_ref()
            .map(|store| store.pending_outbox_count(&mut warnings))
            .unwrap_or(0);

        Self {
            local_device_id: format!("native-{}", identity.public_key_fingerprint),
            room_id: None,
            room_name: None,
            session_role: None,
            ble_role: None,
            scanning: false,
            advertising: false,
            peers: Vec::new(),
            rooms: Vec::new(),
            history,
            native_capabilities: NativeCapabilities::current(),
            native_status: NativeRuntimeStatus {
                sqlite_path: storage
                    .as_ref()
                    .map(|store| store.path.display().to_string()),
                keychain_backed: identity.keychain_backed,
                public_key_fingerprint: identity.public_key_fingerprint,
                last_ble_error: None,
                last_validation: None,
                pending_outbox_events,
            },
            native_warnings: warnings,
        }
    }
}

#[derive(Debug, Clone)]
struct NativeIdentity {
    signing_key: SigningKey,
    keychain_backed: bool,
    public_key_fingerprint: String,
}

#[derive(Debug, Clone)]
struct NativeStorage {
    path: PathBuf,
}

#[derive(Debug, Clone)]
struct OutboxRecord {
    event_id: String,
    payload_json: String,
}

#[napi(object)]
pub struct CreateRoomRequest {
    pub room_name: String,
}

#[napi(object)]
pub struct StartAdvertisingRequest {
    pub room_code: String,
}

#[napi(object)]
pub struct ConnectGuestRequest {
    pub peer_id: String,
}

#[napi(object)]
pub struct JoinRoomRequest {
    pub room_id: String,
}

#[napi(object)]
pub struct SubmitCalculationRequest {
    pub expression: String,
}

#[napi]
pub fn get_state() -> Result<Value> {
    with_state_json(|state| {
        refresh_persisted_history(state);
        Ok(state.clone())
    })
}

#[napi]
pub fn create_room(request: CreateRoomRequest) -> Result<Value> {
    with_state_json(|state| {
        let room_name = trim_or_default(&request.room_name, "Calculator Room");
        state.room_id = Some(format!(
            "room-{}",
            Uuid::new_v4().simple().to_string()[..8].to_string()
        ));
        state.room_name = Some(room_name);
        state.session_role = Some("host".to_string());
        state.ble_role = Some("central".to_string());
        state.scanning = false;
        state.advertising = false;
        state.peers.clear();
        state.rooms.clear();
        Ok(state.clone())
    })
}

#[napi]
pub fn start_scanning() -> Result<Value> {
    with_state_json(|state| {
        state.scanning = true;
        state.advertising = false;
        state.session_role = Some("host".to_string());
        state.ble_role = Some("central".to_string());
        state.native_status.last_ble_error = None;

        match scan_ble_join_requests(state.room_id.as_deref()) {
            Ok(peers) => {
                state.peers = merge_discovered_peers(&state.peers, peers);
                if state.peers.is_empty() {
                    state.native_status.last_ble_error = Some(
                        "No Evolve Calc join advertisements were found for this room.".to_string(),
                    );
                }
            }
            Err(error) => {
                state.native_status.last_ble_error = Some(error.clone());
                push_warning_once(
                    &mut state.native_warnings,
                    format!("BLE central scan attempted but did not complete: {error}"),
                );
            }
        }

        Ok(state.clone())
    })
}

#[napi]
pub fn connect_guest(request: ConnectGuestRequest) -> Result<Value> {
    with_state_json(|state| {
        let mut matched = false;

        if state.session_role.as_deref() != Some("host") {
            push_warning_once(
                &mut state.native_warnings,
                "Only a host/central session can approve guest join requests.",
            );
            return Ok(state.clone());
        }

        let Some(peer_index) = state
            .peers
            .iter()
            .position(|peer| peer.id == request.peer_id)
        else {
            push_warning_once(
                &mut state.native_warnings,
                format!(
                    "Peer {} was not found during native connect request",
                    request.peer_id
                ),
            );
            return Ok(state.clone());
        };

        let connect_result = connect_ble_peer(&request.peer_id);
        for peer in &mut state.peers {
            if peer.id != request.peer_id {
                continue;
            }

            matched = true;
            peer.last_seen_iso = now_iso();
            match &connect_result {
                Ok(()) => {
                    peer.connected = true;
                    peer.trust_status = "trusted".to_string();
                    state.native_status.last_ble_error = None;
                }
                Err(error) => {
                    peer.connected = false;
                    peer.trust_status = "pending".to_string();
                    state.native_status.last_ble_error = Some(error.clone());
                    push_warning_once(
                        &mut state.native_warnings,
                        format!("Native BLE connect failed for {}: {error}", request.peer_id),
                    );
                }
            }
        }

        if !matched {
            let peer = &mut state.peers[peer_index];
            peer.connected = false;
            peer.trust_status = "pending".to_string();
            peer.last_seen_iso = now_iso();
        }

        Ok(state.clone())
    })
}

#[napi]
pub fn scan_rooms() -> Result<Value> {
    with_state_json(|state| {
        state.scanning = true;
        state.advertising = false;
        state.session_role = Some("guest".to_string());
        state.ble_role = Some("central".to_string());
        state.native_status.last_ble_error = None;

        match scan_ble_rooms() {
            Ok(rooms) => {
                state.rooms = merge_discovered_rooms(&state.rooms, rooms);
                if state.rooms.is_empty() {
                    state.native_status.last_ble_error =
                        Some("No Evolve Calc host room advertisements were found.".to_string());
                }
            }
            Err(error) => {
                state.native_status.last_ble_error = Some(error.clone());
                push_warning_once(
                    &mut state.native_warnings,
                    format!("BLE room scan attempted but did not complete: {error}"),
                );
            }
        }

        Ok(state.clone())
    })
}

#[napi]
pub fn join_room(request: JoinRoomRequest) -> Result<Value> {
    with_state_json(|state| {
        let room_id = request.room_id.trim().to_string();
        if room_id.is_empty() {
            push_warning_once(&mut state.native_warnings, "Cannot join an empty room id.");
            return Ok(state.clone());
        }

        let discovered_room = state.rooms.iter().find(|room| room.id == room_id).cloned();
        state.room_id = Some(room_id.clone());
        state.room_name = discovered_room
            .as_ref()
            .map(|room| room.name.clone())
            .or_else(|| Some(format!("Join {room_id}")));
        state.session_role = Some("guest".to_string());
        state.ble_role = Some("peripheral".to_string());
        state.scanning = false;
        state.advertising = true;
        state.peers = vec![PeerSummary {
            id: discovered_room
                .as_ref()
                .map(|room| room.host_device_id.clone())
                .unwrap_or_else(|| "host-native-pending".to_string()),
            label: discovered_room
                .as_ref()
                .map(|room| room.name.clone())
                .unwrap_or_else(|| "Host pending".to_string()),
            session_role: "host".to_string(),
            ble_role: "central".to_string(),
            trust_status: "pending".to_string(),
            connected: false,
            last_seen_iso: now_iso(),
        }];

        push_warning_once(
      &mut state.native_warnings,
      "Guest join request is represented in native state, but real BLE peripheral advertising requires a platform-specific GATT server backend.",
    );

        Ok(state.clone())
    })
}

#[napi]
pub fn start_advertising(request: StartAdvertisingRequest) -> Result<Value> {
    join_room(JoinRoomRequest {
        room_id: request.room_code,
    })
}

#[napi]
pub fn accept_host_connection() -> Result<Value> {
    with_state_json(|state| {
        state.advertising = false;
        state.peers = vec![PeerSummary {
            id: "host-native-pending".to_string(),
            label: "Host connection pending native peripheral backend".to_string(),
            session_role: "host".to_string(),
            ble_role: "central".to_string(),
            trust_status: "pending".to_string(),
            connected: false,
            last_seen_iso: now_iso(),
        }];

        push_warning_once(
      &mut state.native_warnings,
      "Host acceptance is waiting for the platform-specific BLE peripheral/GATT server implementation.",
    );

        Ok(state.clone())
    })
}

#[napi]
pub fn reset_ble_session() -> Result<Value> {
    with_state_json(|state| {
        state.room_id = None;
        state.room_name = None;
        state.session_role = None;
        state.ble_role = None;
        state.scanning = false;
        state.advertising = false;
        state.peers.clear();
        state.rooms.clear();
        state.native_status.last_ble_error = None;
        Ok(state.clone())
    })
}

#[napi]
pub fn submit_calculation(request: SubmitCalculationRequest) -> Result<Value> {
    with_state_json(|state| {
        let expression = request.expression.trim().to_string();
        if expression.is_empty() {
            return Ok(state.clone());
        }

        let identity = load_or_create_identity(&mut state.native_warnings);
        state.local_device_id = format!("native-{}", identity.public_key_fingerprint);
        state.native_status.keychain_backed = identity.keychain_backed;
        state.native_status.public_key_fingerprint = identity.public_key_fingerprint.clone();

        let entry = CalculationEntry {
            id: Uuid::new_v4().to_string(),
            origin_device_id: state.local_device_id.clone(),
            result: calculate_expression(&expression),
            expression,
            trusted: false,
            created_at_iso: now_iso(),
        };

        let envelope = sign_calculation_event(&identity, &entry).map_err(|error| {
            Error::from_reason(format!("Failed to sign calculation event: {error}"))
        })?;
        let validation = validate_signed_calculation_event(&identity, &envelope)?;

        let mut trusted_entry = entry;
        trusted_entry.trusted = validation.valid;
        state.native_status.last_validation = Some(validation);
        state.history.insert(0, trusted_entry.clone());
        state.history.truncate(DEFAULT_HISTORY_LIMIT);

        if let Some(storage) = NativeStorage::open(&mut state.native_warnings) {
            storage.persist_calculation(&trusted_entry, &envelope, &mut state.native_warnings);
            state.native_status.sqlite_path = Some(storage.path.display().to_string());
            consume_sync_outbox(state, &storage);
            refresh_persisted_history(state);
        }

        Ok(state.clone())
    })
}

#[napi]
pub fn get_native_runtime_status() -> Result<Value> {
    with_state_json(|state| {
        let status = json!({
          "capabilities": state.native_capabilities,
          "status": state.native_status,
          "warnings": state.native_warnings,
        });
        Ok(status)
    })
}

#[napi]
pub fn validate_credential_bundle(payload: String) -> Result<Value> {
    let trimmed = payload.trim();
    let summary = if trimmed.is_empty() {
        ValidationSummary::unsupported("empty", "No credential payload was provided")
    } else if trimmed.starts_with('{') {
        ValidationSummary::unsupported(
      "json-credential",
      "JWE/JWT/SD-JWT parsing is intentionally fail-closed until issuer trust configuration is added",
    )
    } else if trimmed.matches('.').count() >= 2 {
        ValidationSummary::unsupported(
      "compact-jose",
      "Compact JOSE payload detected, but issuer trust and key resolution are not configured yet",
    )
    } else {
        ValidationSummary::unsupported("unknown", "Unsupported credential envelope format")
    };

    serde_json::to_value(summary).map_err(|error| {
        Error::from_reason(format!("Failed to serialize validation result: {error}"))
    })
}

fn with_state_json<T>(mut action: impl FnMut(&mut RoomState) -> Result<T>) -> Result<Value>
where
    T: Serialize,
{
    let mut state = APP_STATE
        .lock()
        .map_err(|_| Error::from_reason("Native calculator state lock was poisoned"))?;
    let value = action(&mut state)?;
    serde_json::to_value(value).map_err(|error| {
        Error::from_reason(format!("Failed to serialize native response: {error}"))
    })
}

fn load_or_create_identity(warnings: &mut Vec<String>) -> NativeIdentity {
    match load_keychain_signing_key(warnings) {
        Some(signing_key) => identity_from_signing_key(signing_key, true),
        None => {
            let signing_key = SigningKey::generate(&mut OsRng);
            push_warning_once(
                warnings,
                "Using an in-memory device signing key because OS keychain storage is unavailable.",
            );
            identity_from_signing_key(signing_key, false)
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn load_keychain_signing_key(warnings: &mut Vec<String>) -> Option<SigningKey> {
    let entry = match keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER) {
        Ok(entry) => entry,
        Err(error) => {
            push_warning_once(
                warnings,
                format!("Could not open OS keychain entry: {error}"),
            );
            return None;
        }
    };

    match entry.get_password() {
        Ok(encoded) => match decode_signing_key(&encoded) {
            Some(key) => Some(key),
            None => {
                push_warning_once(
                    warnings,
                    "Stored keychain signing key was invalid; replacing it.",
                );
                create_and_store_signing_key(&entry, warnings)
            }
        },
        Err(_) => create_and_store_signing_key(&entry, warnings),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn load_keychain_signing_key(warnings: &mut Vec<String>) -> Option<SigningKey> {
    push_warning_once(
        warnings,
        "OS keychain integration is not configured for this platform.",
    );
    None
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn create_and_store_signing_key(
    entry: &keyring::Entry,
    warnings: &mut Vec<String>,
) -> Option<SigningKey> {
    let signing_key = SigningKey::generate(&mut OsRng);
    let encoded = URL_SAFE_NO_PAD.encode(signing_key.to_bytes());

    match entry.set_password(&encoded) {
        Ok(()) => Some(signing_key),
        Err(error) => {
            push_warning_once(
                warnings,
                format!("Could not store device key in OS keychain: {error}"),
            );
            None
        }
    }
}

fn decode_signing_key(encoded: &str) -> Option<SigningKey> {
    let bytes = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let key_bytes: [u8; 32] = bytes.try_into().ok()?;
    Some(SigningKey::from_bytes(&key_bytes))
}

fn identity_from_signing_key(signing_key: SigningKey, keychain_backed: bool) -> NativeIdentity {
    let verifying_key = signing_key.verifying_key();
    NativeIdentity {
        signing_key,
        keychain_backed,
        public_key_fingerprint: fingerprint_public_key(&verifying_key),
    }
}

fn fingerprint_public_key(verifying_key: &VerifyingKey) -> String {
    let digest = Sha256::digest(verifying_key.as_bytes());
    to_hex(&digest[..12])
}

impl NativeStorage {
    fn open(warnings: &mut Vec<String>) -> Option<Self> {
        let Some(path) = app_database_path(warnings) else {
            return None;
        };

        let storage = Self { path };
        if let Err(error) = storage.initialize() {
            push_warning_once(warnings, format!("SQLite initialization failed: {error}"));
            None
        } else {
            Some(storage)
        }
    }

    fn initialize(&self) -> rusqlite::Result<()> {
        let connection = Connection::open(&self.path)?;
        connection.execute_batch(
            "
      PRAGMA journal_mode = WAL;
      CREATE TABLE IF NOT EXISTS calculations (
        id TEXT PRIMARY KEY,
        origin_device_id TEXT NOT NULL,
        expression TEXT NOT NULL,
        result TEXT NOT NULL,
        trusted INTEGER NOT NULL,
        created_at_iso TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS sync_outbox (
        id TEXT PRIMARY KEY,
        event_id TEXT NOT NULL,
        payload_json TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at_iso TEXT NOT NULL
      );
      CREATE TABLE IF NOT EXISTS trusted_issuers (
        issuer TEXT PRIMARY KEY,
        public_key_fingerprint TEXT NOT NULL,
        created_at_iso TEXT NOT NULL
      );
      ",
        )?;
        Ok(())
    }

    fn load_history(
        &self,
        limit: usize,
        warnings: &mut Vec<String>,
    ) -> Option<Vec<CalculationEntry>> {
        let connection = match Connection::open(&self.path) {
            Ok(connection) => connection,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite open failed while loading history: {error}"),
                );
                return None;
            }
        };

        let mut statement = match connection.prepare(
            "
      SELECT id, origin_device_id, expression, result, trusted, created_at_iso
      FROM calculations
      ORDER BY created_at_iso DESC
      LIMIT ?1
      ",
        ) {
            Ok(statement) => statement,
            Err(error) => {
                push_warning_once(warnings, format!("SQLite history query failed: {error}"));
                return None;
            }
        };

        let rows = match statement.query_map(params![limit as i64], |row| {
            Ok(CalculationEntry {
                id: row.get(0)?,
                origin_device_id: row.get(1)?,
                expression: row.get(2)?,
                result: row.get(3)?,
                trusted: row.get::<_, i64>(4)? == 1,
                created_at_iso: row.get(5)?,
            })
        }) {
            Ok(rows) => rows,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite history row mapping failed: {error}"),
                );
                return None;
            }
        };

        let mut entries = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => entries.push(entry),
                Err(error) => {
                    push_warning_once(warnings, format!("SQLite history row failed: {error}"))
                }
            }
        }

        Some(entries)
    }

    fn persist_calculation(
        &self,
        entry: &CalculationEntry,
        envelope: &SignedEnvelope,
        warnings: &mut Vec<String>,
    ) {
        let payload_json = match serde_json::to_string(envelope) {
            Ok(payload_json) => payload_json,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("Failed to serialize sync outbox payload: {error}"),
                );
                return;
            }
        };

        let connection = match Connection::open(&self.path) {
            Ok(connection) => connection,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite open failed while persisting event: {error}"),
                );
                return;
            }
        };

        if let Err(error) = connection.execute(
            "
      INSERT OR REPLACE INTO calculations
        (id, origin_device_id, expression, result, trusted, created_at_iso)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6)
      ",
            params![
                entry.id,
                entry.origin_device_id,
                entry.expression,
                entry.result,
                if entry.trusted { 1 } else { 0 },
                entry.created_at_iso
            ],
        ) {
            push_warning_once(
                warnings,
                format!("SQLite calculation persist failed: {error}"),
            );
            return;
        }

        if let Err(error) = connection.execute(
            "
      INSERT OR REPLACE INTO sync_outbox
        (id, event_id, payload_json, status, created_at_iso)
      VALUES (?1, ?2, ?3, 'pending', ?4)
      ",
            params![
                Uuid::new_v4().to_string(),
                entry.id,
                payload_json,
                now_iso()
            ],
        ) {
            push_warning_once(warnings, format!("SQLite outbox persist failed: {error}"));
        }
    }

    fn pending_outbox_count(&self, warnings: &mut Vec<String>) -> usize {
        let connection = match Connection::open(&self.path) {
            Ok(connection) => connection,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite open failed while counting outbox: {error}"),
                );
                return 0;
            }
        };

        match connection.query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE status = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(count) => count.max(0) as usize,
            Err(error) => {
                push_warning_once(warnings, format!("SQLite outbox count failed: {error}"));
                0
            }
        }
    }

    fn load_pending_outbox(&self, limit: usize, warnings: &mut Vec<String>) -> Vec<OutboxRecord> {
        let connection = match Connection::open(&self.path) {
            Ok(connection) => connection,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite open failed while loading outbox: {error}"),
                );
                return Vec::new();
            }
        };

        let mut statement = match connection.prepare(
            "
      SELECT event_id, payload_json
      FROM sync_outbox
      WHERE status = 'pending'
      ORDER BY created_at_iso ASC
      LIMIT ?1
      ",
        ) {
            Ok(statement) => statement,
            Err(error) => {
                push_warning_once(warnings, format!("SQLite outbox query failed: {error}"));
                return Vec::new();
            }
        };

        let rows = match statement.query_map(params![limit as i64], |row| {
            Ok(OutboxRecord {
                event_id: row.get(0)?,
                payload_json: row.get(1)?,
            })
        }) {
            Ok(rows) => rows,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite outbox row mapping failed: {error}"),
                );
                return Vec::new();
            }
        };

        let mut records = Vec::new();
        for row in rows {
            match row {
                Ok(record) => records.push(record),
                Err(error) => {
                    push_warning_once(warnings, format!("SQLite outbox row failed: {error}"))
                }
            }
        }

        records
    }
}

fn refresh_persisted_history(state: &mut RoomState) {
    if let Some(storage) = NativeStorage::open(&mut state.native_warnings) {
        if let Some(history) =
            storage.load_history(DEFAULT_HISTORY_LIMIT, &mut state.native_warnings)
        {
            state.history = history;
        }
        state.native_status.pending_outbox_events =
            storage.pending_outbox_count(&mut state.native_warnings);
        state.native_status.sqlite_path = Some(storage.path.display().to_string());
    }
}

fn app_database_path(warnings: &mut Vec<String>) -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        env::var_os("APPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
    };

    let Some(dir) = base.map(|path| path.join("BleCalculator")) else {
        push_warning_once(
            warnings,
            "Could not determine app data directory for SQLite.",
        );
        return None;
    };

    if let Err(error) = fs::create_dir_all(&dir) {
        push_warning_once(
            warnings,
            format!("Could not create app data directory: {error}"),
        );
        return None;
    }

    Some(dir.join("evolve-calc.sqlite3"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedEnvelope {
    protected: String,
    payload: String,
    signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct BleTransportChunk {
    message_id: String,
    index: u16,
    total: u16,
    payload_b64: String,
}

fn sign_calculation_event(
    identity: &NativeIdentity,
    entry: &CalculationEntry,
) -> std::result::Result<SignedEnvelope, String> {
    let protected = json!({
      "alg": "EdDSA",
      "typ": "calc-event+jws",
      "kid": identity.public_key_fingerprint,
    });
    let payload = serde_json::to_value(entry).map_err(|error| error.to_string())?;
    let protected_b64 =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&protected).map_err(|error| error.to_string())?);
    let payload_b64 =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).map_err(|error| error.to_string())?);
    let signing_input = format!("{protected_b64}.{payload_b64}");
    let signature = identity.signing_key.sign(signing_input.as_bytes());

    Ok(SignedEnvelope {
        protected: protected_b64,
        payload: payload_b64,
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

fn validate_signed_calculation_event(
    identity: &NativeIdentity,
    envelope: &SignedEnvelope,
) -> Result<ValidationSummary> {
    let signing_input = format!("{}.{}", envelope.protected, envelope.payload);
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(&envelope.signature)
        .map_err(|error| {
            Error::from_reason(format!(
                "Invalid calculation JWS signature encoding: {error}"
            ))
        })?;
    let signature_array: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| Error::from_reason("Invalid calculation JWS signature length"))?;
    let signature = Signature::from_bytes(&signature_array);
    identity
        .signing_key
        .verifying_key()
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|error| {
            Error::from_reason(format!("Calculation JWS verification failed: {error}"))
        })?;

    let payload_bytes = URL_SAFE_NO_PAD.decode(&envelope.payload).map_err(|error| {
        Error::from_reason(format!("Invalid calculation JWS payload encoding: {error}"))
    })?;
    let entry: CalculationEntry = serde_json::from_slice(&payload_bytes)
        .map_err(|error| Error::from_reason(format!("Invalid calculation JWS payload: {error}")))?;

    let expected_device_id = format!("native-{}", identity.public_key_fingerprint);
    if entry.origin_device_id != expected_device_id {
        return Err(Error::from_reason(
            "Calculation event failed holder key binding",
        ));
    }

    Ok(ValidationSummary::local_event_valid(
        "Local calculation event signature and holder binding verified",
    ))
}

fn consume_sync_outbox(state: &mut RoomState, storage: &NativeStorage) {
    state.native_status.pending_outbox_events =
        storage.pending_outbox_count(&mut state.native_warnings);

    if state.native_status.pending_outbox_events == 0 {
        return;
    }

    if !state.peers.iter().any(|peer| peer.connected) {
        return;
    }

    let records = storage.load_pending_outbox(10, &mut state.native_warnings);
    let chunk_count: usize = records
        .iter()
        .map(|record| {
            let chunks = chunk_payload(record.event_id.clone(), record.payload_json.as_bytes());
            if let Err(error) = reassemble_chunks(&chunks) {
                push_warning_once(
                    &mut state.native_warnings,
                    format!("BLE chunk staging failed local reassembly check: {error}"),
                );
            }
            chunks.len()
        })
        .sum();

    if chunk_count > 0 {
        state.native_status.last_ble_error = Some(format!(
      "BLE event transport is not available yet; {} pending outbox event(s) staged into {} chunk(s) and retained.",
      records.len(),
      chunk_count
    ));
    }
}

fn chunk_payload(message_id: String, payload: &[u8]) -> Vec<BleTransportChunk> {
    if payload.is_empty() {
        return vec![BleTransportChunk {
            message_id,
            index: 0,
            total: 1,
            payload_b64: String::new(),
        }];
    }

    let total = payload
        .chunks(BLE_CHUNK_PAYLOAD_SIZE)
        .count()
        .min(u16::MAX as usize) as u16;

    payload
        .chunks(BLE_CHUNK_PAYLOAD_SIZE)
        .take(total as usize)
        .enumerate()
        .map(|(index, chunk)| BleTransportChunk {
            message_id: message_id.clone(),
            index: index as u16,
            total,
            payload_b64: URL_SAFE_NO_PAD.encode(chunk),
        })
        .collect()
}

fn reassemble_chunks(chunks: &[BleTransportChunk]) -> std::result::Result<Vec<u8>, String> {
    let Some(first) = chunks.first() else {
        return Err("No BLE chunks were provided".to_string());
    };

    let message_id = &first.message_id;
    let total = first.total;
    if total == 0 || chunks.len() != total as usize {
        return Err("BLE chunk set is incomplete".to_string());
    }

    let mut ordered = chunks.to_vec();
    ordered.sort_by_key(|chunk| chunk.index);

    let mut reassembled = Vec::new();
    for (expected_index, chunk) in ordered.iter().enumerate() {
        if &chunk.message_id != message_id {
            return Err("BLE chunks contain mixed message ids".to_string());
        }
        if chunk.total != total || chunk.index != expected_index as u16 {
            return Err("BLE chunks are out of sequence".to_string());
        }

        let mut bytes = URL_SAFE_NO_PAD
            .decode(&chunk.payload_b64)
            .map_err(|error| format!("BLE chunk payload was not base64url encoded: {error}"))?;
        reassembled.append(&mut bytes);
    }

    Ok(reassembled)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BleAdvertisementPayload {
    kind: String,
    room_id: Option<String>,
    room_name: Option<String>,
    device_id: Option<String>,
    label: Option<String>,
}

fn scan_ble_join_requests(
    room_filter: Option<&str>,
) -> std::result::Result<Vec<PeerSummary>, String> {
    let advertisements = scan_ble_calculator_advertisements()?;
    Ok(advertisements
        .into_iter()
        .filter_map(|(address, payload)| peer_from_advertisement(address, payload, room_filter))
        .collect())
}

fn scan_ble_rooms() -> std::result::Result<Vec<RoomSummary>, String> {
    let advertisements = scan_ble_calculator_advertisements()?;
    Ok(advertisements
        .into_iter()
        .filter_map(|(address, payload)| room_from_advertisement(address, payload))
        .collect())
}

fn scan_ble_calculator_advertisements(
) -> std::result::Result<Vec<(String, BleAdvertisementPayload)>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| format!("Tokio runtime unavailable: {error}"))?;

    runtime.block_on(async {
        let manager = Manager::new()
            .await
            .map_err(|error| format!("BLE manager unavailable: {error}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|error| format!("BLE adapter list unavailable: {error}"))?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No BLE adapter found".to_string())?;

        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|error| format!("BLE scan failed to start: {error}"))?;
        tokio::time::sleep(Duration::from_millis(2200)).await;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("BLE peripheral list unavailable: {error}"))?;
        let _ = adapter.stop_scan().await;

        let mut advertisements = Vec::new();
        for peripheral in peripherals {
            let properties = match peripheral.properties().await {
                Ok(Some(properties)) => properties,
                Ok(None) => continue,
                Err(_) => continue,
            };
            let address = properties.address.to_string();
            if let Some(payload) = parse_calculator_advertisement(&properties) {
                advertisements.push((address, payload));
            }
        }

        Ok(advertisements)
    })
}

fn connect_ble_peer(peer_id: &str) -> std::result::Result<(), String> {
    let Some(address) = peer_id.strip_prefix("ble-") else {
        return Err("Peer is not a native BLE discovery result".to_string());
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| format!("Tokio runtime unavailable: {error}"))?;

    runtime.block_on(async {
        let manager = Manager::new()
            .await
            .map_err(|error| format!("BLE manager unavailable: {error}"))?;
        let adapters = manager
            .adapters()
            .await
            .map_err(|error| format!("BLE adapter list unavailable: {error}"))?;
        let adapter = adapters
            .into_iter()
            .next()
            .ok_or_else(|| "No BLE adapter found".to_string())?;

        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|error| format!("BLE scan failed before connect: {error}"))?;
        tokio::time::sleep(Duration::from_millis(900)).await;

        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|error| format!("BLE peripheral list unavailable before connect: {error}"))?;
        let _ = adapter.stop_scan().await;

        for peripheral in peripherals {
            let properties = match peripheral.properties().await {
                Ok(Some(properties)) => properties,
                _ => continue,
            };

            if properties.address.to_string() != address {
                continue;
            }

            peripheral
                .connect()
                .await
                .map_err(|error| format!("BLE peripheral connect failed: {error}"))?;
            peripheral
                .discover_services()
                .await
                .map_err(|error| format!("BLE service discovery failed: {error}"))?;
            return Ok(());
        }

        Err(format!("BLE peer {address} was not found for connect"))
    })
}

fn parse_calculator_advertisement(
    properties: &PeripheralProperties,
) -> Option<BleAdvertisementPayload> {
    let service_uuid = Uuid::parse_str(CALCULATOR_SERVICE_UUID).ok()?;
    if let Some(data) = properties.service_data.get(&service_uuid) {
        if let Ok(payload) = serde_json::from_slice::<BleAdvertisementPayload>(data) {
            return Some(payload);
        }
    }

    properties
        .local_name
        .as_ref()
        .and_then(|name| parse_local_name_advertisement(name))
}

fn parse_local_name_advertisement(name: &str) -> Option<BleAdvertisementPayload> {
    let mut parts = name.splitn(4, ':');
    let prefix = parts.next()?;
    if prefix != ADVERTISEMENT_PREFIX {
        return None;
    }

    let kind = parts.next()?.trim().to_string();
    let room_id = parts
        .next()
        .map(str::trim)
        .filter(|part| !part.is_empty())?
        .to_string();
    let label = parts
        .next()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string);

    Some(BleAdvertisementPayload {
        kind,
        room_id: Some(room_id),
        room_name: label.clone(),
        device_id: None,
        label,
    })
}

fn peer_from_advertisement(
    address: String,
    payload: BleAdvertisementPayload,
    room_filter: Option<&str>,
) -> Option<PeerSummary> {
    if payload.kind.to_ascii_uppercase() != JOIN_ADVERTISEMENT_KIND {
        return None;
    }

    if let (Some(expected_room), Some(advertised_room)) = (room_filter, payload.room_id.as_deref())
    {
        if expected_room != advertised_room {
            return None;
        }
    }

    Some(PeerSummary {
        id: format!("ble-{address}"),
        label: payload
            .label
            .or(payload.device_id)
            .unwrap_or_else(|| format!("Evolve Calc Guest {address}")),
        session_role: "guest".to_string(),
        ble_role: "peripheral".to_string(),
        trust_status: "pending".to_string(),
        connected: false,
        last_seen_iso: now_iso(),
    })
}

fn room_from_advertisement(
    address: String,
    payload: BleAdvertisementPayload,
) -> Option<RoomSummary> {
    if payload.kind.to_ascii_uppercase() != ROOM_ADVERTISEMENT_KIND {
        return None;
    }

    let room_id = payload.room_id?;
    Some(RoomSummary {
        id: room_id.clone(),
        name: payload
            .room_name
            .or(payload.label)
            .unwrap_or_else(|| format!("Room {room_id}")),
        host_device_id: payload
            .device_id
            .unwrap_or_else(|| format!("ble-{address}")),
        trust_status: "pending".to_string(),
        joinable: true,
        last_seen_iso: now_iso(),
    })
}

fn merge_discovered_peers(
    existing: &[PeerSummary],
    discovered: Vec<PeerSummary>,
) -> Vec<PeerSummary> {
    let mut merged = existing.to_vec();
    for peer in discovered {
        match merged.iter_mut().find(|candidate| candidate.id == peer.id) {
            Some(existing_peer) => {
                existing_peer.label = peer.label;
                existing_peer.last_seen_iso = peer.last_seen_iso;
            }
            None => merged.push(peer),
        }
    }
    merged
}

fn merge_discovered_rooms(
    existing: &[RoomSummary],
    discovered: Vec<RoomSummary>,
) -> Vec<RoomSummary> {
    let mut merged = existing.to_vec();
    for room in discovered {
        match merged.iter_mut().find(|candidate| candidate.id == room.id) {
            Some(existing_room) => {
                existing_room.name = room.name;
                existing_room.host_device_id = room.host_device_id;
                existing_room.trust_status = room.trust_status;
                existing_room.joinable = room.joinable;
                existing_room.last_seen_iso = room.last_seen_iso;
            }
            None => merged.push(room),
        }
    }
    merged
}

fn trim_or_default(value: &str, default_value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default_value.to_string()
    } else {
        trimmed.to_string()
    }
}

fn now_iso() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn calculate_expression(expression: &str) -> String {
    match parse_expression(expression) {
        Some(value) if value.is_finite() => {
            let rounded = (value * 100_000_000.0).round() / 100_000_000.0;
            format!("{rounded}")
        }
        _ => "Invalid expression".to_string(),
    }
}

fn parse_expression(expression: &str) -> Option<f64> {
    let sanitized: String = expression
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if sanitized.is_empty() {
        return None;
    }

    let tokens = tokenize(&sanitized)?;
    let mut values: Vec<f64> = Vec::new();
    let mut ops: Vec<char> = Vec::new();

    for token in tokens {
        match token {
            Token::Number(value) => values.push(value),
            Token::Op(op) => {
                while ops
                    .last()
                    .is_some_and(|existing| precedence(*existing) >= precedence(op))
                {
                    apply_op(&mut values, ops.pop()?)?;
                }
                ops.push(op);
            }
        }
    }

    while let Some(op) = ops.pop() {
        apply_op(&mut values, op)?;
    }

    if values.len() == 1 {
        values.pop()
    } else {
        None
    }
}

#[derive(Debug)]
enum Token {
    Number(f64),
    Op(char),
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    let mut expects_number = true;

    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && expects_number) {
            let mut value = String::new();
            if ch == '-' {
                value.push(chars.next()?);
            }

            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() || next == '.' {
                    value.push(chars.next()?);
                } else {
                    break;
                }
            }

            tokens.push(Token::Number(value.parse().ok()?));
            expects_number = false;
            continue;
        }

        if matches!(ch, '+' | '-' | '*' | '/' | '%') && !expects_number {
            tokens.push(Token::Op(chars.next()?));
            expects_number = true;
            continue;
        }

        return None;
    }

    if expects_number {
        return None;
    }

    Some(tokens)
}

fn precedence(op: char) -> u8 {
    match op {
        '+' | '-' => 1,
        '*' | '/' | '%' => 2,
        _ => 0,
    }
}

fn apply_op(values: &mut Vec<f64>, op: char) -> Option<()> {
    let right = values.pop()?;
    let left = values.pop()?;
    let result = match op {
        '+' => left + right,
        '-' => left - right,
        '*' => left * right,
        '/' if right != 0.0 => left / right,
        '%' if right != 0.0 => left % right,
        _ => return None,
    };
    values.push(result);
    Some(())
}

fn push_warning_once(warnings: &mut Vec<String>, warning: impl Into<String>) {
    let warning = warning.into();
    if !warnings.iter().any(|existing| existing == &warning) {
        warnings.push(warning);
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[allow(dead_code)]
fn is_database_path(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "evolve-calc.sqlite3")
}

#[cfg(test)]
mod tests {
    use super::{
        calculate_expression, chunk_payload, is_database_path, merge_discovered_peers,
        parse_local_name_advertisement, peer_from_advertisement, reassemble_chunks,
        room_from_advertisement, PeerSummary,
    };
    use std::path::Path;

    #[test]
    fn evaluates_operator_precedence() {
        assert_eq!(calculate_expression("7 + 5 * 2"), "17");
    }

    #[test]
    fn rejects_invalid_expression() {
        assert_eq!(calculate_expression("7 + nope"), "Invalid expression");
    }

    #[test]
    fn evaluates_modulo() {
        assert_eq!(calculate_expression("10 % 4"), "2");
    }

    #[test]
    fn recognizes_app_database_path() {
        assert!(is_database_path(Path::new("/tmp/evolve-calc.sqlite3")));
    }

    #[test]
    fn preserves_connected_peer_when_scan_updates_existing_peer() {
        let existing = vec![PeerSummary {
            id: "ble-1".to_string(),
            label: "Old".to_string(),
            session_role: "guest".to_string(),
            ble_role: "peripheral".to_string(),
            trust_status: "trusted".to_string(),
            connected: true,
            last_seen_iso: "old".to_string(),
        }];
        let discovered = vec![PeerSummary {
            id: "ble-1".to_string(),
            label: "New".to_string(),
            session_role: "guest".to_string(),
            ble_role: "peripheral".to_string(),
            trust_status: "pending".to_string(),
            connected: false,
            last_seen_iso: "new".to_string(),
        }];

        let merged = merge_discovered_peers(&existing, discovered);

        assert_eq!(merged[0].label, "New");
        assert!(merged[0].connected);
        assert_eq!(merged[0].trust_status, "trusted");
    }

    #[test]
    fn parses_join_request_local_name() {
        let payload = parse_local_name_advertisement("EvolveCalc:JOIN:room-123:MacBook Guest")
            .expect("advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, Some("room-123"))
            .expect("join request should become a peer");

        assert_eq!(peer.id, "ble-AA-BB");
        assert_eq!(peer.label, "MacBook Guest");
        assert_eq!(peer.session_role, "guest");
    }

    #[test]
    fn rejects_join_request_for_another_room() {
        let payload = parse_local_name_advertisement("EvolveCalc:JOIN:room-abc:Guest")
            .expect("advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, Some("room-other"));

        assert!(peer.is_none());
    }

    #[test]
    fn parses_room_local_name() {
        let payload = parse_local_name_advertisement("EvolveCalc:ROOM:room-abc:Desk Calculator")
            .expect("advertisement should parse");
        let room = room_from_advertisement("AA-BB".to_string(), payload)
            .expect("room advertisement should become a room");

        assert_eq!(room.id, "room-abc");
        assert_eq!(room.name, "Desk Calculator");
        assert_eq!(room.host_device_id, "ble-AA-BB");
    }

    #[test]
    fn reassembles_ble_chunks() {
        let payload = b"calculation event payload that is longer than a single tiny test chunk";
        let chunks = chunk_payload("event-1".to_string(), payload);
        let reassembled = reassemble_chunks(&chunks).expect("chunks should reassemble");

        assert_eq!(reassembled, payload);
        assert!(chunks.iter().all(|chunk| chunk.message_id == "event-1"));
    }
}

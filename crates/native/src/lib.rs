use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use btleplug::api::PeripheralProperties;
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
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

mod ble;
mod ble_central;

const DEFAULT_HISTORY_LIMIT: usize = 100;
const KEYCHAIN_SERVICE: &str = "io.evolve.ble-calculator";
const KEYCHAIN_USER: &str = "device-signing-key-v1";
const CALCULATOR_SERVICE_UUID: &str = "7c14f94a-77dd-4a65-9f04-6f7ac8d2a601";
/// Current advertisement prefix (`EVC:<kind>:<room>`). Terse on purpose: the
/// whole local name must fit the 29-byte scan-response budget or CoreBluetooth
/// truncates it and the room id is corrupted. Kept in sync with `ble/mod.rs`.
const ADVERTISEMENT_PREFIX: &str = "EVC";
/// Prefix of the original 4-field `EvolveCalc:<KIND>:<room>:<label>` format.
/// Still parsed so builds that predate the short format stay discoverable.
const LEGACY_ADVERTISEMENT_PREFIX: &str = "EvolveCalc";
const JOIN_ADVERTISEMENT_KIND: &str = "JOIN";
const ROOM_ADVERTISEMENT_KIND: &str = "ROOM";
const BLE_CHUNK_PAYLOAD_SIZE: usize = 180;
/// Hard ceiling for one serialized transport frame. Core Bluetooth's automatic
/// long-write supports up to 512 bytes with write-with-response, so a framed
/// chunk (`BleTransportChunk` JSON: base64 payload + message-id UUID + counters)
/// must stay under this. `BLE_CHUNK_PAYLOAD_SIZE` is sized to leave headroom;
/// `frame_event_for_ble` asserts it and `worst_case_frame_fits_mtu_budget` tests it.
const BLE_MAX_FRAME_BYTES: usize = 512;
/// Notify frame budget used while the subscribed host's real
/// `maximumUpdateValueLength` is still unknown (frames queued before a host
/// subscribes). Unlike write-with-response, notifications get **no** automatic
/// fragmentation, so this matches the small limit seen on conservative BLE
/// 4.x links; a known larger limit replaces it in `deliver_event_to_host`.
const BLE_CONSERVATIVE_NOTIFY_FRAME_BYTES: usize = 182;
/// Worst-case byte overhead of one serialized `BleTransportChunk` frame around
/// its base64 payload (JSON keys, a UUID message id, and 5-digit counters).
/// `chunk_frame_overhead_is_not_underestimated` keeps this honest.
const BLE_CHUNK_FRAME_OVERHEAD_BYTES: usize = 100;
/// Cap on concurrently-reassembling inbound messages, so a peer that streams
/// incomplete chunk sets cannot grow the reassembly buffer without bound.
const BLE_MAX_INFLIGHT_MESSAGES: usize = 64;

static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));

/// Partially-received inbound chunk sets, keyed by `message_id`. A message is
/// removed and processed once all `total` chunks have arrived. Kept out of
/// `RoomState` so partial transport framing never leaks into the UI contract.
static BLE_REASSEMBLY: Lazy<Mutex<HashMap<String, Vec<BleTransportChunk>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// The platform BLE peripheral backend (guest advertising / GATT server).
/// `btleplug` cannot fill the peripheral role, so this is a per-OS native
/// implementation behind the [`ble::BlePeripheral`] trait.
static PERIPHERAL: Lazy<Mutex<Box<dyn ble::BlePeripheral>>> =
    Lazy::new(|| Mutex::new(ble::new_peripheral()));

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
    /// Most recent RSSI in dBm from the discovery scan, or `None` when the
    /// backend did not report one. Serialized as `rssi` for the renderer.
    #[serde(skip_serializing_if = "Option::is_none")]
    rssi: Option<i16>,
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
    /// See [`PeerSummary::rssi`].
    #[serde(skip_serializing_if = "Option::is_none")]
    rssi: Option<i16>,
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
            // A native peripheral/GATT-server backend exists on macOS
            // (CoreBluetooth), Linux (BlueZ/bluer), and Windows
            // (GattServiceProvider); other platforms fall back to the fail-loud
            // stub (see `ble`).
            ble_peripheral_advertising: cfg!(any(
                target_os = "macos",
                target_os = "linux",
                target_os = "windows"
            )),
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

// Retained for the reconnect-replay follow-up: `persist_calculation` still fills
// the outbox table, and `load_pending_outbox` reads it. Live delivery currently
// goes through the GATT write path in `deliver_event_to_guest`.
#[allow(dead_code)]
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
        refresh_connection_state(state);
        process_incoming_ble(state);
        Ok(state.clone())
    })
}

/// Drain and process calculation events received over BLE since the last poll.
/// Host (central) reads guest -> host TX notifications; guest (peripheral) reads
/// host -> guest characteristic writes. Both are reassembled, signature- and
/// holder-binding-verified, and appended to history. This is the receive half
/// that closes the two end-to-end gaps (host->guest #1 and guest->host).
fn process_incoming_ble(state: &mut RoomState) {
    let role = state.session_role.clone();
    let role = role.as_deref();

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut host_subscribed = false;
    // Asynchronous transport failures (failed advertising, undeliverable or
    // dropped notify frames, drain errors) surface here on every poll instead
    // of being swallowed inside the backends.
    let mut transport_error: Option<String> = None;
    let mut peripheral_advertising = state.advertising;

    match role {
        Some("guest") => {
            if let Ok(mut peripheral) = PERIPHERAL.lock() {
                frames.extend(peripheral.take_inbound());
                host_subscribed = peripheral.has_subscriber();
                transport_error = peripheral.take_last_error();
                peripheral_advertising = peripheral.is_advertising();
            }
        }
        Some("host") => {
            // The host also runs the peripheral backend (ROOM discovery
            // beacon), and its advertising confirms asynchronously — a beacon
            // that failed after `create_room` returned would otherwise fail
            // silently and no guest could ever discover this room.
            if let Ok(mut peripheral) = PERIPHERAL.lock() {
                if let Some(error) = peripheral.take_last_error() {
                    transport_error = Some(format!("Room discovery beacon error: {error}"));
                }
            }
            match ble_central::take_notifications() {
                Ok(notified) => frames.extend(notified),
                Err(error) => transport_error = Some(error),
            }
        }
        _ => return,
    }

    if let Some(error) = transport_error {
        state.native_status.last_ble_error = Some(error);
        // Reconcile the optimistic advertising flag with the backend's
        // delegate-confirmed state (e.g. didStartAdvertising failed after
        // `join_room` already returned Ok).
        if role == Some("guest") {
            state.advertising = peripheral_advertising;
        }
    }

    let received = ingest_ble_frames(frames, &mut state.native_warnings);

    if !received.is_empty() {
        if let Some(storage) = NativeStorage::open(&mut state.native_warnings) {
            for entry in &received {
                let mut stored = entry.clone();
                stored.trusted = true;
                storage.persist_received_calculation(&stored, &mut state.native_warnings);
            }
        }
        // A verified event proves the peer holds the signing key it claims, so
        // the live link is now a trusted, connected session (findings #3, #5).
        note_verified_peer(state);
        refresh_persisted_history(state);
    } else if role == Some("guest") && host_subscribed {
        // No data yet, but a host has subscribed: surface the live link so the
        // guest UI leaves the "waiting" state (finding #5).
        mark_host_connected(state);
    }
}

/// Parse, buffer, reassemble, and verify inbound BLE frames. Returns the fully
/// received-and-verified calculation events. Malformed frames and events that
/// fail signature/holder-binding verification are dropped with a warning.
fn ingest_ble_frames(frames: Vec<Vec<u8>>, warnings: &mut Vec<String>) -> Vec<CalculationEntry> {
    let mut completed = Vec::new();
    if frames.is_empty() {
        return completed;
    }

    let mut buffers = match BLE_REASSEMBLY.lock() {
        Ok(buffers) => buffers,
        Err(_) => return completed,
    };

    for frame in frames {
        let chunk: BleTransportChunk = match serde_json::from_slice(&frame) {
            Ok(chunk) => chunk,
            Err(error) => {
                push_warning_once(warnings, format!("Dropped malformed inbound BLE frame: {error}"));
                continue;
            }
        };

        if chunk.total == 0 {
            continue;
        }

        // Bound the in-flight set: refuse new message ids past the cap so a peer
        // streaming incomplete sets cannot grow the buffer without bound.
        if !buffers.contains_key(&chunk.message_id) && buffers.len() >= BLE_MAX_INFLIGHT_MESSAGES {
            push_warning_once(
                warnings,
                "Too many incomplete inbound BLE messages; dropping new chunk.",
            );
            continue;
        }

        let total = chunk.total;
        let message_id = chunk.message_id.clone();
        let entry = buffers.entry(message_id.clone()).or_default();
        // Replace a resent chunk at the same index instead of duplicating it, so
        // `len() == total` reliably means "every distinct index present".
        match entry.iter_mut().find(|existing| existing.index == chunk.index) {
            Some(existing) => *existing = chunk,
            None => entry.push(chunk),
        }

        if entry.len() as u16 != total {
            continue;
        }

        let chunks = buffers.remove(&message_id).unwrap_or_default();
        match reassemble_chunks(&chunks) {
            Ok(bytes) => match verify_received_calculation_event(&bytes) {
                Ok(event) => completed.push(event),
                Err(error) => push_warning_once(
                    warnings,
                    format!("Rejected received BLE calculation event: {error}"),
                ),
            },
            Err(error) => push_warning_once(
                warnings,
                format!("Failed to reassemble received BLE chunks: {error}"),
            ),
        }
    }

    completed
}

/// Reflect a verified peer on the current session: trust the live peer and, for
/// a guest, mark the host connected. Trust is now earned by a verified signed
/// event rather than asserted on bare connection (finding #3).
fn note_verified_peer(state: &mut RoomState) {
    match state.session_role.as_deref() {
        Some("host") => {
            for peer in &mut state.peers {
                if peer.connected {
                    peer.trust_status = "trusted".to_string();
                    peer.last_seen_iso = now_iso();
                }
            }
        }
        Some("guest") => {
            for peer in &mut state.peers {
                if peer.session_role == "host" {
                    peer.connected = true;
                    peer.trust_status = "trusted".to_string();
                    peer.last_seen_iso = now_iso();
                }
            }
        }
        _ => {}
    }
}

/// Mark the guest's host peer connected without promoting trust (used when the
/// host has subscribed but no verified event has arrived yet).
fn mark_host_connected(state: &mut RoomState) {
    for peer in &mut state.peers {
        if peer.session_role == "host" && !peer.connected {
            peer.connected = true;
            peer.last_seen_iso = now_iso();
        }
    }
}

/// Poll-based liveness for the host's retained central connection. If the state
/// shows a connected guest but the BLE link has dropped, reflect the drop so the
/// UI stops showing a stale Connection Card (review gap #3). Only applies to the
/// host (central) — a guest's link is owned by the peripheral backend, not here.
fn refresh_connection_state(state: &mut RoomState) {
    if state.session_role.as_deref() != Some("host") {
        return;
    }
    if !state.peers.iter().any(|peer| peer.connected) {
        return;
    }
    if ble_central::is_connected() {
        return;
    }

    for peer in &mut state.peers {
        if peer.connected {
            peer.connected = false;
            if peer.trust_status == "trusted" {
                peer.trust_status = "pending".to_string();
            }
        }
    }
    state.scanning = false;
    state.native_status.last_ble_error = Some("Guest peer disconnected.".to_string());
}

#[napi]
pub fn create_room(request: CreateRoomRequest) -> Result<Value> {
    with_state_json(|state| {
        let room_name = trim_or_default(&request.room_name, "Calculator Room");
        // Short on purpose: this id rides the BLE advertisement local name
        // (`EVC:R:<id>`, 29-byte scan-response budget) and doubles as the join
        // code a guest types by hand.
        let room_id = format!("r-{}", &Uuid::new_v4().simple().to_string()[..6]);
        state.room_id = Some(room_id.clone());
        state.room_name = Some(room_name.clone());
        state.session_role = Some("host".to_string());
        state.ble_role = Some("central".to_string());
        state.scanning = false;
        state.advertising = false;
        state.peers.clear();
        state.rooms.clear();
        state.native_status.last_ble_error = None;

        // Advertise a ROOM discovery beacon so guests scanning for hosts
        // (`scan_rooms`) can find this room. Non-fatal here, but the failure is
        // surfaced as `lastBleError` (not just a warning): without the beacon a
        // guest can only reach this room by typing the join code, so the host
        // must see that discovery is degraded. Late/async failures (advertising
        // confirms after this returns) surface via the host-role
        // `take_last_error` poll in `process_incoming_ble`.
        if let Err(error) = start_room_advertising(&room_id) {
            let message = format!("Room discovery beacon could not start: {error}");
            state.native_status.last_ble_error = Some(message.clone());
            push_warning_once(&mut state.native_warnings, message);
        }

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
            Ok(scan) => {
                // An empty result is a normal outcome (no guest is advertising a
                // JOIN yet), not a BLE error — the renderer keeps re-running the
                // one-shot scan until a guest appears, so stay quiet here. But
                // "devices seen, none usable" is worth flagging: it means a
                // nearby advertiser matched the calculator service UUID while
                // its name failed to parse or named a different room.
                // Names are included so a failed E2E shows exactly what was on
                // air; `push_warning_once` still dedups because the visible
                // device set is stable across the renderer's 3 s rescan pump.
                if scan.items.is_empty() && scan.raw_device_count > 0 {
                    push_warning_once(
                        &mut state.native_warnings,
                        format!(
                            "BLE scan saw device(s) advertising the calculator service, but none matched a joinable guest for this room. Seen names: {}",
                            format_seen_device_names(&scan.raw_device_names)
                        ),
                    );
                }
                state.peers = merge_discovered_peers(&state.peers, scan.items);
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
                    // Trust is *not* asserted on a bare GATT connection. It is
                    // earned once a signed calculation event from this peer
                    // passes signature + holder-binding verification in
                    // `process_incoming_ble` (finding #3).
                    peer.trust_status = "pending".to_string();
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

        // A successful connection ends discovery: the UI swaps the Discovery
        // list for the Connection Card, so scanning must stop here too.
        if connect_result.is_ok() {
            state.scanning = false;
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
            Ok(scan) => {
                // An empty result is a normal outcome (no host is advertising a
                // ROOM beacon in range yet), not a BLE error — the renderer keeps
                // re-running the one-shot scan until a room appears, so stay
                // quiet here. "Devices seen, none usable" is flagged though: it
                // points at an unparseable or non-ROOM advertisement nearby.
                // Names are included so a failed E2E shows exactly what was on
                // air; `push_warning_once` still dedups because the visible
                // device set is stable across the renderer's 3 s rescan pump.
                if scan.items.is_empty() && scan.raw_device_count > 0 {
                    push_warning_once(
                        &mut state.native_warnings,
                        format!(
                            "BLE scan saw device(s) advertising the calculator service, but none advertised a joinable room. Seen names: {}",
                            format_seen_device_names(&scan.raw_device_names)
                        ),
                    );
                }
                state.rooms = merge_discovered_rooms(&state.rooms, scan.items);
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
        // Normalize hand-typed join codes: ids are generated lowercase and the
        // advertisement builder lowercases too, so case must never matter.
        let room_id = request.room_id.trim().to_ascii_lowercase();
        if room_id.is_empty() {
            push_warning_once(&mut state.native_warnings, "Cannot join an empty room id.");
            return Ok(state.clone());
        }
        // An oversized code would overflow the 29-byte scan-response budget and
        // be truncated on air, so the host could never match it. Reject it
        // up front instead of advertising a corrupted id.
        if room_id.len() > ble::MAX_ROOM_ID_BYTES {
            let message = format!(
                "Room code {room_id:?} is too long to advertise over BLE (max {} characters).",
                ble::MAX_ROOM_ID_BYTES
            );
            state.native_status.last_ble_error = Some(message.clone());
            push_warning_once(&mut state.native_warnings, message);
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
            rssi: None,
        }];

        match start_guest_advertising(&room_id) {
            Ok(()) => {
                state.native_status.last_ble_error = None;
            }
            Err(error) => {
                state.advertising = false;
                state.native_status.last_ble_error = Some(error.clone());
                push_warning_once(
                    &mut state.native_warnings,
                    format!("Guest BLE advertising could not start: {error}"),
                );
            }
        }

        Ok(state.clone())
    })
}

fn start_guest_advertising(room_id: &str) -> std::result::Result<(), String> {
    let config = ble::PeripheralConfig::join(room_id);
    let mut peripheral = PERIPHERAL
        .lock()
        .map_err(|_| "BLE peripheral backend lock was poisoned".to_string())?;
    peripheral.start_advertising(&config)
}

/// Advertise a host ROOM discovery beacon (`EVC:R:<room>`) via the peripheral
/// backend so guests scanning for rooms can find this host.
fn start_room_advertising(room_id: &str) -> std::result::Result<(), String> {
    let config = ble::PeripheralConfig::room(room_id);
    let mut peripheral = PERIPHERAL
        .lock()
        .map_err(|_| "BLE peripheral backend lock was poisoned".to_string())?;
    peripheral.start_advertising(&config)
}

fn stop_guest_advertising() {
    if let Ok(mut peripheral) = PERIPHERAL.lock() {
        let _ = peripheral.stop();
    }
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
        let backend_supported = peripheral_backend_supported();

        // On a guest peripheral the host initiates the GATT connection, so
        // "accept" keeps advertising active and reports the current backend
        // rather than fabricating a connected host.
        let (label, warning) = if backend_supported {
            (
                "Advertising for host connection".to_string(),
                None,
            )
        } else {
            (
                "Host connection pending native peripheral backend".to_string(),
                Some(
                    "Host acceptance needs this platform's BLE peripheral/GATT server backend, which is not implemented yet."
                        .to_string(),
                ),
            )
        };

        state.peers = vec![PeerSummary {
            id: "host-native-pending".to_string(),
            label,
            session_role: "host".to_string(),
            ble_role: "central".to_string(),
            trust_status: "pending".to_string(),
            connected: false,
            last_seen_iso: now_iso(),
            rssi: None,
        }];

        if let Some(warning) = warning {
            state.advertising = false;
            push_warning_once(&mut state.native_warnings, warning);
        }

        Ok(state.clone())
    })
}

fn peripheral_backend_supported() -> bool {
    PERIPHERAL
        .lock()
        .map(|peripheral| peripheral.is_supported())
        .unwrap_or(false)
}

/// Diagnostic snapshot of the guest peripheral backend for the runtime status.
fn peripheral_status_json() -> Value {
    match PERIPHERAL.lock() {
        Ok(peripheral) => json!({
            "platform": peripheral.platform(),
            "supported": peripheral.is_supported(),
            "advertising": peripheral.is_advertising(),
        }),
        Err(_) => json!({
            "platform": "unavailable",
            "supported": false,
            "advertising": false,
        }),
    }
}

/// Discard any un-consumed inbound frames and partially-reassembled messages
/// when resetting the session, so a new session never inherits stale bytes.
fn drain_peripheral_inbound() -> usize {
    if let Ok(mut buffers) = BLE_REASSEMBLY.lock() {
        buffers.clear();
    }
    match PERIPHERAL.lock() {
        Ok(mut peripheral) => peripheral.take_inbound().len(),
        Err(_) => 0,
    }
}

#[napi]
pub fn reset_ble_session() -> Result<Value> {
    stop_guest_advertising();
    ble_central::disconnect();
    let _drained = drain_peripheral_inbound();
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

        // Live cross-device delivery: a host writes to the connected guest
        // (host -> guest), a guest notifies the subscribed host (guest -> host).
        match state.session_role.as_deref() {
            Some("host") => deliver_event_to_guest(state, &trusted_entry, &envelope),
            Some("guest") => deliver_event_to_host(state, &trusted_entry, &envelope),
            _ => {}
        }

        Ok(state.clone())
    })
}

/// Stream a freshly-signed calculation event to a connected guest over the real
/// GATT write path (review gap #1). No-op unless we are the host with a live
/// central connection. On a dropped link it reflects the disconnect instead of
/// silently failing.
fn deliver_event_to_guest(
    state: &mut RoomState,
    entry: &CalculationEntry,
    envelope: &SignedEnvelope,
) {
    if state.session_role.as_deref() != Some("host") {
        return;
    }
    if !state.peers.iter().any(|peer| peer.connected) {
        return;
    }

    if !ble_central::is_connected() {
        refresh_connection_state(state);
        return;
    }

    let frames = match frame_event_for_ble(entry.id.clone(), envelope, BLE_MAX_FRAME_BYTES) {
        Ok(frames) => frames,
        Err(error) => {
            state.native_status.last_ble_error = Some(error);
            return;
        }
    };

    match ble_central::write_frames(&frames) {
        Ok(_) => state.native_status.last_ble_error = None,
        Err(error) => state.native_status.last_ble_error = Some(error),
    }
}

/// Guest -> host delivery: notify the freshly-signed event to a subscribed host
/// over the TX characteristic. Frames are queued by the peripheral backend even
/// if no host has subscribed yet, and flushed once one does — so a guest can
/// calculate before the host connects without losing events.
fn deliver_event_to_host(
    state: &mut RoomState,
    entry: &CalculationEntry,
    envelope: &SignedEnvelope,
) {
    if state.session_role.as_deref() != Some("guest") {
        return;
    }

    let result = match PERIPHERAL.lock() {
        Ok(mut peripheral) => {
            // Notifications are never fragmented by Core Bluetooth, so size each
            // frame to the subscribed host's actual `maximumUpdateValueLength`.
            // Before a host subscribes that limit is unknown; use the
            // conservative budget so queued frames stay deliverable on any link.
            let frame_budget = peripheral
                .max_notify_frame_len()
                .unwrap_or(BLE_CONSERVATIVE_NOTIFY_FRAME_BYTES)
                .min(BLE_MAX_FRAME_BYTES);
            frame_event_for_ble(entry.id.clone(), envelope, frame_budget)
                .and_then(|frames| peripheral.notify(&frames))
        }
        Err(_) => Err("BLE peripheral backend lock was poisoned".to_string()),
    };
    match result {
        Ok(_) => state.native_status.last_ble_error = None,
        Err(error) => state.native_status.last_ble_error = Some(error),
    }
}

/// Serialize a signed envelope, chunk it to fit `frame_budget`, and frame each
/// chunk for BLE transport. Shared by the host->guest write path (budget =
/// `BLE_MAX_FRAME_BYTES`, Core Bluetooth long-write) and the guest->host notify
/// path (budget = the subscriber's `maximumUpdateValueLength`, since
/// notifications are never fragmented).
fn frame_event_for_ble(
    message_id: String,
    envelope: &SignedEnvelope,
    frame_budget: usize,
) -> std::result::Result<Vec<Vec<u8>>, String> {
    let chunk_size = chunk_payload_size_for_frame_budget(frame_budget).ok_or_else(|| {
        format!("BLE frame budget of {frame_budget} bytes is too small to carry chunked events")
    })?;
    let payload_json = serde_json::to_vec(envelope)
        .map_err(|error| format!("Failed to serialize calculation event for BLE: {error}"))?;
    let chunks = chunk_payload_sized(message_id, &payload_json, chunk_size);
    let frames: std::result::Result<Vec<Vec<u8>>, _> =
        chunks.iter().map(serde_json::to_vec).collect();
    let frames =
        frames.map_err(|error| format!("Failed to frame calculation chunks for BLE: {error}"))?;
    // Real check (not just a debug assert): an oversized frame would be
    // truncated or rejected by the transport, so refuse to send it at all.
    if let Some(oversized) = frames.iter().find(|frame| frame.len() > frame_budget) {
        return Err(format!(
            "Framed BLE chunk is {} bytes, exceeding the {frame_budget}-byte transport budget",
            oversized.len()
        ));
    }
    Ok(frames)
}

#[napi]
pub fn get_native_runtime_status() -> Result<Value> {
    with_state_json(|state| {
        let status = json!({
          "capabilities": state.native_capabilities,
          "status": state.native_status,
          "warnings": state.native_warnings,
          "peripheral": peripheral_status_json(),
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

    /// Persist a calculation event received from a peer over BLE. Unlike
    /// [`persist_calculation`] this does **not** enqueue a sync-outbox row: a
    /// received event is recorded locally but never re-broadcast by this device.
    fn persist_received_calculation(&self, entry: &CalculationEntry, warnings: &mut Vec<String>) {
        let connection = match Connection::open(&self.path) {
            Ok(connection) => connection,
            Err(error) => {
                push_warning_once(
                    warnings,
                    format!("SQLite open failed while persisting received event: {error}"),
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
                format!("SQLite received-calculation persist failed: {error}"),
            );
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

    #[allow(dead_code)] // Retained for reconnect-replay; see OutboxRecord.
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
    // Embed the public key (`jwk`) so a *receiving* device can verify the
    // signature and holder binding without a pre-shared key or PKI: it verifies
    // with the embedded key, then checks the origin device id equals the
    // fingerprint of that key. `kid` remains the fingerprint for quick display.
    let public_key_b64 =
        URL_SAFE_NO_PAD.encode(identity.signing_key.verifying_key().as_bytes());
    let protected = json!({
      "alg": "EdDSA",
      "typ": "calc-event+jws",
      "kid": identity.public_key_fingerprint,
      "jwk": public_key_b64,
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

/// Verify a `SignedEnvelope` received from another device. Unlike
/// [`validate_signed_calculation_event`], the signer is a *peer*, so the key is
/// taken from the envelope's embedded `jwk` and holder binding is checked
/// against the fingerprint of that same key. Returns the verified event.
fn verify_received_calculation_event(
    payload: &[u8],
) -> std::result::Result<CalculationEntry, String> {
    let envelope: SignedEnvelope = serde_json::from_slice(payload)
        .map_err(|error| format!("Invalid received signed envelope: {error}"))?;

    let protected_bytes = URL_SAFE_NO_PAD
        .decode(&envelope.protected)
        .map_err(|error| format!("Invalid protected header encoding: {error}"))?;
    let protected: Value = serde_json::from_slice(&protected_bytes)
        .map_err(|error| format!("Invalid protected header: {error}"))?;
    let jwk_b64 = protected
        .get("jwk")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "Received event is missing the embedded public key".to_string())?;
    let key_bytes = URL_SAFE_NO_PAD
        .decode(jwk_b64)
        .map_err(|error| format!("Invalid embedded public key encoding: {error}"))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "Invalid embedded public key length".to_string())?;
    let verifying_key = VerifyingKey::from_bytes(&key_array)
        .map_err(|error| format!("Invalid embedded public key: {error}"))?;

    let signing_input = format!("{}.{}", envelope.protected, envelope.payload);
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(&envelope.signature)
        .map_err(|error| format!("Invalid received signature encoding: {error}"))?;
    let signature_array: [u8; 64] = signature_bytes
        .try_into()
        .map_err(|_| "Invalid received signature length".to_string())?;
    let signature = Signature::from_bytes(&signature_array);
    verifying_key
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|error| format!("Received event signature verification failed: {error}"))?;

    let payload_bytes = URL_SAFE_NO_PAD
        .decode(&envelope.payload)
        .map_err(|error| format!("Invalid received payload encoding: {error}"))?;
    let entry: CalculationEntry = serde_json::from_slice(&payload_bytes)
        .map_err(|error| format!("Invalid received payload: {error}"))?;

    let expected_device_id = format!("native-{}", fingerprint_public_key(&verifying_key));
    if entry.origin_device_id != expected_device_id {
        return Err("Received event failed holder key binding".to_string());
    }

    Ok(entry)
}

fn consume_sync_outbox(state: &mut RoomState, storage: &NativeStorage) {
    // Live delivery to a connected guest now happens in `deliver_event_to_guest`
    // via the real GATT write path. The SQLite outbox remains a durable record of
    // events; draining/acking it over BLE (for reconnect replay) is a separate
    // follow-up, so here we only refresh the pending counter for diagnostics.
    state.native_status.pending_outbox_events =
        storage.pending_outbox_count(&mut state.native_warnings);
}

fn chunk_payload_sized(
    message_id: String,
    payload: &[u8],
    chunk_size: usize,
) -> Vec<BleTransportChunk> {
    if payload.is_empty() {
        return vec![BleTransportChunk {
            message_id,
            index: 0,
            total: 1,
            payload_b64: String::new(),
        }];
    }

    let total = payload.chunks(chunk_size).count().min(u16::MAX as usize) as u16;

    payload
        .chunks(chunk_size)
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

/// How many raw payload bytes fit in one chunk whose serialized frame must stay
/// within `frame_budget` bytes: budget minus the JSON envelope overhead, scaled
/// down by the 4/3 base64 inflation. `None` when the budget cannot fit even a
/// single payload byte.
fn chunk_payload_size_for_frame_budget(frame_budget: usize) -> Option<usize> {
    let b64_budget = frame_budget.checked_sub(BLE_CHUNK_FRAME_OVERHEAD_BYTES)?;
    // (b64_budget / 4) * 3 raw bytes encode to a multiple of 4 base64 chars
    // that never exceeds b64_budget.
    let raw = (b64_budget / 4) * 3;
    if raw == 0 {
        return None;
    }
    Some(raw.min(BLE_CHUNK_PAYLOAD_SIZE))
}

#[allow(dead_code)] // Used by unit tests and the guest-side reassembly follow-up.
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

/// Outcome of one discovery scan: the summaries that survived parsing and
/// filtering, plus what the radio actually saw. The raw count and names let
/// callers distinguish "nothing on air" from "seen but unparseable/filtered"
/// when `items` comes back empty — and show exactly which names failed.
struct ScanOutcome<T> {
    items: Vec<T>,
    raw_device_count: usize,
    /// Advertised names of every raw device (`<unnamed>` when absent),
    /// truncated for log hygiene. Diagnostic only.
    raw_device_names: Vec<String>,
}

/// Render the raw device names for a diagnostic warning.
fn format_seen_device_names(names: &[String]) -> String {
    names.join(", ")
}

fn scan_ble_join_requests(
    room_filter: Option<&str>,
) -> std::result::Result<ScanOutcome<PeerSummary>, String> {
    let scan = scan_ble_calculator_advertisements()?;
    Ok(ScanOutcome {
        items: scan
            .items
            .into_iter()
            .filter_map(|(address, payload, rssi)| {
                peer_from_advertisement(address, payload, rssi, room_filter)
            })
            .collect(),
        raw_device_count: scan.raw_device_count,
        raw_device_names: scan.raw_device_names,
    })
}

fn scan_ble_rooms() -> std::result::Result<ScanOutcome<RoomSummary>, String> {
    let scan = scan_ble_calculator_advertisements()?;
    Ok(ScanOutcome {
        items: scan
            .items
            .into_iter()
            .filter_map(|(address, payload, rssi)| room_from_advertisement(address, payload, rssi))
            .collect(),
        raw_device_count: scan.raw_device_count,
        raw_device_names: scan.raw_device_names,
    })
}

/// Service-filtered scan via the persistent central. Each result carries the
/// device address, the parsed calculator advertisement, and the RSSI (dBm).
fn scan_ble_calculator_advertisements(
) -> std::result::Result<ScanOutcome<(String, BleAdvertisementPayload, Option<i16>)>, String> {
    let service_uuid = Uuid::parse_str(CALCULATOR_SERVICE_UUID)
        .map_err(|error| format!("Invalid calculator service UUID: {error}"))?;

    let properties = ble_central::scan(service_uuid, 2200)?;
    let raw_device_count = properties.len();
    let raw_device_names = properties
        .iter()
        .map(|props| match props.local_name.as_deref() {
            Some(name) => name.chars().take(64).collect(),
            None => "<unnamed>".to_string(),
        })
        .collect();
    Ok(ScanOutcome {
        items: properties
            .into_iter()
            .filter_map(|props| {
                let address = props.address.to_string();
                let rssi = props.rssi;
                parse_calculator_advertisement(&props).map(|payload| (address, payload, rssi))
            })
            .collect(),
        raw_device_count,
        raw_device_names,
    })
}

fn connect_ble_peer(peer_id: &str) -> std::result::Result<(), String> {
    let Some(address) = peer_id.strip_prefix("ble-") else {
        return Err("Peer is not a native BLE discovery result".to_string());
    };

    let service_uuid = Uuid::parse_str(CALCULATOR_SERVICE_UUID)
        .map_err(|error| format!("Invalid calculator service UUID: {error}"))?;
    let rx_uuid = Uuid::parse_str(ble::CALCULATOR_RX_CHARACTERISTIC_UUID)
        .map_err(|error| format!("Invalid calculator RX characteristic UUID: {error}"))?;
    let tx_uuid = Uuid::parse_str(ble::CALCULATOR_TX_CHARACTERISTIC_UUID)
        .map_err(|error| format!("Invalid calculator TX characteristic UUID: {error}"))?;

    ble_central::connect(address, service_uuid, rx_uuid, tx_uuid)
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

/// Locate the calculator advertisement payload inside a scanned device name.
///
/// The payload is usually the whole name, but btleplug's CoreBluetooth backend
/// merges the GAP device name with the advertised local name into
/// `"<device name> [<local name>]"` (btleplug `internal.rs`,
/// `on_discovered_peripheral`) — so a Mac beacon arrives as e.g.
/// `"Sofia's MacBook Pro [EVC:R:r-790b50]"`. Find the payload wherever it
/// starts and drop a trailing `]` from that wrapping.
fn extract_advertisement_payload(name: &str) -> Option<&str> {
    for prefix in [ADVERTISEMENT_PREFIX, LEGACY_ADVERTISEMENT_PREFIX] {
        let marker = format!("{prefix}:");
        if let Some(start) = name.find(&marker) {
            let payload = &name[start..];
            let payload = payload.strip_suffix(']').unwrap_or(payload);
            return Some(payload.trim());
        }
    }
    None
}

/// Parse both advertisement generations: the current 3-field `EVC:<K>:<room>`
/// (kinds `J`/`R`) and the legacy 4-field `EvolveCalc:<KIND>:<room>:<label>`
/// (kinds `JOIN`/`ROOM`, label optional because scan responses truncated it).
/// The payload may be embedded in a btleplug-merged device name — see
/// [`extract_advertisement_payload`].
fn parse_local_name_advertisement(name: &str) -> Option<BleAdvertisementPayload> {
    let payload = extract_advertisement_payload(name)?;
    let mut parts = payload.splitn(4, ':');
    let prefix = parts.next()?;
    if prefix != ADVERTISEMENT_PREFIX && prefix != LEGACY_ADVERTISEMENT_PREFIX {
        return None;
    }

    let kind = match parts.next()?.trim() {
        "J" => JOIN_ADVERTISEMENT_KIND,
        "R" => ROOM_ADVERTISEMENT_KIND,
        other => other,
    }
    .to_string();
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
    rssi: Option<i16>,
    room_filter: Option<&str>,
) -> Option<PeerSummary> {
    if payload.kind.to_ascii_uppercase() != JOIN_ADVERTISEMENT_KIND {
        return None;
    }

    if let (Some(expected_room), Some(advertised_room)) = (room_filter, payload.room_id.as_deref())
    {
        // Join codes are typed by hand on the guest, so compare them
        // whitespace- and case-insensitively.
        if !expected_room
            .trim()
            .eq_ignore_ascii_case(advertised_room.trim())
        {
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
        rssi,
    })
}

fn room_from_advertisement(
    address: String,
    payload: BleAdvertisementPayload,
    rssi: Option<i16>,
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
        rssi,
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
                existing_peer.rssi = peer.rssi;
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
                existing_room.rssi = room.rssi;
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
        calculate_expression, chunk_payload_sized, frame_event_for_ble, identity_from_signing_key,
        is_database_path, merge_discovered_peers, parse_local_name_advertisement,
        peer_from_advertisement,
        reassemble_chunks, room_from_advertisement, sign_calculation_event,
        verify_received_calculation_event, CalculationEntry, PeerSummary, SigningKey,
        BLE_CHUNK_FRAME_OVERHEAD_BYTES, BLE_CHUNK_PAYLOAD_SIZE, BLE_MAX_FRAME_BYTES,
    };
    use rand_core::OsRng;
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
            rssi: None,
        }];
        let discovered = vec![PeerSummary {
            id: "ble-1".to_string(),
            label: "New".to_string(),
            session_role: "guest".to_string(),
            ble_role: "peripheral".to_string(),
            trust_status: "pending".to_string(),
            connected: false,
            last_seen_iso: "new".to_string(),
            rssi: Some(-72),
        }];

        let merged = merge_discovered_peers(&existing, discovered);

        assert_eq!(merged[0].label, "New");
        assert!(merged[0].connected);
        assert_eq!(merged[0].trust_status, "trusted");
        // A re-scan refreshes RSSI while preserving connection/trust.
        assert_eq!(merged[0].rssi, Some(-72));
    }

    #[test]
    fn parses_short_join_local_name() {
        let payload = parse_local_name_advertisement("EVC:J:r-abc123")
            .expect("advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, Some(-58), Some("r-abc123"))
            .expect("join request should become a peer");

        assert_eq!(peer.id, "ble-AA-BB");
        assert_eq!(peer.rssi, Some(-58));
        assert_eq!(peer.session_role, "guest");
    }

    #[test]
    fn matches_room_filter_ignoring_case_and_whitespace() {
        // The join code is typed by hand on the guest; the host filter must
        // still match when the case or surrounding whitespace differs.
        let payload = parse_local_name_advertisement("EVC:J:r-abc123")
            .expect("advertisement should parse");
        let peer =
            peer_from_advertisement("AA-BB".to_string(), payload, None, Some(" R-ABC123 "));

        assert!(peer.is_some());
    }

    #[test]
    fn parses_legacy_join_request_local_name() {
        let payload = parse_local_name_advertisement("EvolveCalc:JOIN:room-123:MacBook Guest")
            .expect("advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, Some(-58), Some("room-123"))
            .expect("join request should become a peer");

        assert_eq!(peer.id, "ble-AA-BB");
        assert_eq!(peer.label, "MacBook Guest");
        assert_eq!(peer.rssi, Some(-58));
        assert_eq!(peer.session_role, "guest");
    }

    #[test]
    fn parses_legacy_join_local_name_with_truncated_label() {
        // A 29-byte scan response cut the legacy label off; the peer must still
        // be discoverable with a synthesized label.
        let payload = parse_local_name_advertisement("EvolveCalc:JOIN:room-123")
            .expect("truncated advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, None, Some("room-123"))
            .expect("join request should become a peer");

        assert_eq!(peer.label, "Evolve Calc Guest AA-BB");
    }

    #[test]
    fn rejects_join_request_for_another_room() {
        let payload = parse_local_name_advertisement("EvolveCalc:JOIN:room-abc:Guest")
            .expect("advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, None, Some("room-other"));

        assert!(peer.is_none());
    }

    #[test]
    fn parses_short_room_local_name() {
        let payload = parse_local_name_advertisement("EVC:R:r-abc123")
            .expect("advertisement should parse");
        let room = room_from_advertisement("AA-BB".to_string(), payload, Some(-61))
            .expect("room advertisement should become a room");

        assert_eq!(room.id, "r-abc123");
        assert_eq!(room.rssi, Some(-61));
        // No label travels on the wire; the display name is synthesized.
        assert_eq!(room.name, "Room r-abc123");
        assert_eq!(room.host_device_id, "ble-AA-BB");
    }

    #[test]
    fn parses_legacy_room_local_name() {
        let payload = parse_local_name_advertisement("EvolveCalc:ROOM:room-abc:Desk Calculator")
            .expect("advertisement should parse");
        let room = room_from_advertisement("AA-BB".to_string(), payload, Some(-61))
            .expect("room advertisement should become a room");

        assert_eq!(room.id, "room-abc");
        assert_eq!(room.rssi, Some(-61));
        assert_eq!(room.name, "Desk Calculator");
        assert_eq!(room.host_device_id, "ble-AA-BB");
    }

    #[test]
    fn parses_payload_embedded_in_btleplug_merged_name() {
        // btleplug's CoreBluetooth backend reports "<device name> [<local
        // name>]" when a peripheral has both, so a Mac beacon arrives wrapped
        // in the computer name. Both directions must still parse.
        let payload =
            parse_local_name_advertisement("Sofia's MacBook Pro [EVC:R:r-790b50]")
                .expect("merged room advertisement should parse");
        let room = room_from_advertisement("AA-BB".to_string(), payload, None)
            .expect("room advertisement should become a room");
        assert_eq!(room.id, "r-790b50");

        let payload = parse_local_name_advertisement("Guest MacBook [EVC:J:r-790b50]")
            .expect("merged join advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, None, Some("r-790b50"));
        assert!(peer.is_some());
    }

    #[test]
    fn parses_legacy_payload_embedded_in_btleplug_merged_name() {
        let payload = parse_local_name_advertisement(
            "MacBook Pro [EvolveCalc:JOIN:room-123:MacBook Guest]",
        )
        .expect("merged legacy advertisement should parse");
        let peer = peer_from_advertisement("AA-BB".to_string(), payload, None, Some("room-123"))
            .expect("join request should become a peer");
        // Only the merged-name wrapping `]` is stripped, not label content.
        assert_eq!(peer.label, "MacBook Guest");
    }

    #[test]
    fn rejects_unrelated_local_name() {
        assert!(parse_local_name_advertisement("SomeHeadphones").is_none());
        assert!(parse_local_name_advertisement("EVCX:J:r-abc123").is_none());
        // A bare device name with no embedded payload must not parse.
        assert!(parse_local_name_advertisement("Sofia's MacBook Pro").is_none());
    }

    fn test_identity() -> super::NativeIdentity {
        identity_from_signing_key(SigningKey::generate(&mut OsRng), false)
    }

    fn test_entry(identity: &super::NativeIdentity) -> CalculationEntry {
        CalculationEntry {
            id: "evt-round-trip".to_string(),
            origin_device_id: format!("native-{}", identity.public_key_fingerprint),
            expression: "2 + 2".to_string(),
            result: "4".to_string(),
            trusted: false,
            created_at_iso: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn verifies_received_signed_event_round_trip() {
        let identity = test_identity();
        let entry = test_entry(&identity);
        let envelope = sign_calculation_event(&identity, &entry).expect("sign");
        let bytes = serde_json::to_vec(&envelope).expect("serialize envelope");

        let verified = verify_received_calculation_event(&bytes).expect("verify");
        assert_eq!(verified.id, "evt-round-trip");
        assert_eq!(verified.result, "4");
        assert_eq!(verified.origin_device_id, entry.origin_device_id);
    }

    #[test]
    fn rejects_received_event_whose_origin_is_not_the_signer() {
        let identity = test_identity();
        // Sign a self-consistent envelope, but claim a different origin device:
        // the signature verifies, holder binding must not.
        let mut entry = test_entry(&identity);
        entry.origin_device_id = "native-someoneelse".to_string();
        let envelope = sign_calculation_event(&identity, &entry).expect("sign");
        let bytes = serde_json::to_vec(&envelope).expect("serialize envelope");

        let error = verify_received_calculation_event(&bytes).expect_err("must reject");
        assert!(error.contains("holder key binding"), "unexpected error: {error}");
    }

    #[test]
    fn rejects_received_event_with_tampered_payload() {
        let identity = test_identity();
        let entry = test_entry(&identity);
        let mut envelope = sign_calculation_event(&identity, &entry).expect("sign");
        // Flip the payload so the signature no longer matches.
        envelope.payload.push_str("AA");
        let bytes = serde_json::to_vec(&envelope).expect("serialize envelope");

        let error = verify_received_calculation_event(&bytes).expect_err("must reject");
        assert!(
            error.contains("signature verification failed") || error.contains("payload"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn worst_case_frame_fits_mtu_budget() {
        // A full-size chunk with a UUID-length message id is the largest single
        // frame we ever put on the wire; it must stay within the 512-byte budget.
        let message_id = "123e4567-e89b-12d3-a456-426614174000".to_string();
        let payload = vec![0xABu8; BLE_CHUNK_PAYLOAD_SIZE];
        let chunks = chunk_payload_sized(message_id, &payload, BLE_CHUNK_PAYLOAD_SIZE);
        assert_eq!(chunks.len(), 1);
        for chunk in &chunks {
            let frame = serde_json::to_vec(chunk).expect("frame");
            assert!(
                frame.len() <= BLE_MAX_FRAME_BYTES,
                "framed chunk is {} bytes, over the {BLE_MAX_FRAME_BYTES} budget",
                frame.len()
            );
        }
    }

    #[test]
    fn rejects_frames_over_the_transport_budget() {
        // The payload is chunked to 180 bytes, so the only way a frame can
        // exceed the budget is per-frame overhead — an oversized message id
        // simulates that. Must be a hard error in release builds, not a
        // debug-only assert.
        let identity = test_identity();
        let entry = test_entry(&identity);
        let envelope = sign_calculation_event(&identity, &entry).expect("sign");

        let oversized_message_id = "x".repeat(BLE_MAX_FRAME_BYTES);
        let error = frame_event_for_ble(oversized_message_id, &envelope, BLE_MAX_FRAME_BYTES)
            .expect_err("oversized frames must be rejected");
        assert!(
            error.contains("transport budget"),
            "unexpected error: {error}"
        );

        // Sanity: a normal UUID message id frames fine.
        frame_event_for_ble(entry.id.clone(), &envelope, BLE_MAX_FRAME_BYTES)
            .expect("normal frames fit");
    }

    #[test]
    fn chunk_frame_overhead_is_not_underestimated() {
        // Worst-case envelope: UUID-length message id, 5-digit counters, empty
        // payload. Everything but the base64 payload must fit in the overhead
        // constant, or `chunk_payload_size_for_frame_budget` over-fills frames.
        let chunk = super::BleTransportChunk {
            message_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
            index: u16::MAX,
            total: u16::MAX,
            payload_b64: String::new(),
        };
        let frame = serde_json::to_vec(&chunk).expect("frame");
        assert!(
            frame.len() <= BLE_CHUNK_FRAME_OVERHEAD_BYTES,
            "empty-payload frame is {} bytes, over the {BLE_CHUNK_FRAME_OVERHEAD_BYTES}-byte overhead allowance",
            frame.len()
        );
    }

    #[test]
    fn budget_sized_frames_fit_small_notify_limits() {
        // Notifications are never fragmented, so framing must adapt to small
        // `maximumUpdateValueLength` values, not just the 512-byte write budget.
        let identity = test_identity();
        let entry = test_entry(&identity);
        let envelope = sign_calculation_event(&identity, &entry).expect("sign");

        for budget in [super::BLE_CONSERVATIVE_NOTIFY_FRAME_BYTES, 120, 256, 512] {
            let frames = frame_event_for_ble(entry.id.clone(), &envelope, budget)
                .unwrap_or_else(|error| panic!("framing failed for budget {budget}: {error}"));
            assert!(!frames.is_empty());
            for frame in &frames {
                assert!(
                    frame.len() <= budget,
                    "frame is {} bytes, over the {budget}-byte budget",
                    frame.len()
                );
            }
        }

        // A budget too small for even one payload byte must fail loudly.
        let error = frame_event_for_ble(entry.id.clone(), &envelope, 100)
            .expect_err("tiny budgets must be rejected");
        assert!(error.contains("too small"), "unexpected error: {error}");
    }

    #[test]
    fn reassembles_ble_chunks() {
        let payload = b"calculation event payload that is longer than a single tiny test chunk";
        let chunks =
            chunk_payload_sized("event-1".to_string(), payload, BLE_CHUNK_PAYLOAD_SIZE);
        let reassembled = reassemble_chunks(&chunks).expect("chunks should reassemble");

        assert_eq!(reassembled, payload);
        assert!(chunks.iter().all(|chunk| chunk.message_id == "event-1"));
    }
}

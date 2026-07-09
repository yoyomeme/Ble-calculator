//! macOS guest BLE peripheral backend built on CoreBluetooth's
//! `CBPeripheralManager` via the `objc2` bindings.
//!
//! Threading model: CoreBluetooth objects are not `Send`, and CB delivers all
//! delegate callbacks on a dispatch queue. We create one dedicated **serial**
//! dispatch queue and confine every manager/service/characteristic access to
//! it. Commands coming from the calculator thread (`start_advertising`/`stop`)
//! are marshalled onto that same queue with `exec_async`, so there is only ever
//! one thread touching the non-`Send` objects. The `AssertSend` wrapper encodes
//! that invariant for the type system.
//!
//! Runtime validation of this backend requires real hardware (two Macs, the
//! `NSBluetoothAlwaysUsageDescription` entitlement, and a granted Bluetooth
//! permission), so it is exercised structurally here and on-device separately.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_bluetooth::{
    CBATTError, CBATTRequest, CBAdvertisementDataLocalNameKey, CBAdvertisementDataServiceUUIDsKey,
    CBAttributePermissions, CBCentral, CBCharacteristic, CBCharacteristicProperties, CBManagerState,
    CBMutableCharacteristic, CBMutableService, CBPeripheralManager, CBPeripheralManagerDelegate,
    CBService, CBUUID,
};
use objc2_foundation::{NSArray, NSData, NSError, NSMutableDictionary, NSString};

use super::{BlePeripheral, PeripheralConfig};

/// Upper bound on queued outbound messages retained for a not-yet-subscribed
/// host. A guest can calculate before the host connects, but an absent host
/// must not grow the queue without bound; the oldest whole messages are dropped
/// (and surfaced as an error) past this cap. Message-granular so a drop never
/// leaves a half-sent chunk set the receiver can't reassemble; mirrors the
/// inbound `BLE_MAX_INFLIGHT_MESSAGES` cap.
const MAX_OUTBOUND_MESSAGES: usize = 64;

/// Confines a non-`Send` value to the serial CB queue. Sound because every
/// access happens on that single queue.
struct AssertSend<T>(T);
// SAFETY: values are only ever dereferenced on the serial dispatch queue.
unsafe impl<T> Send for AssertSend<T> {}

/// State shared between the calculator-facing handle, the delegate, and the
/// blocks dispatched onto the CB queue.
struct Shared {
    advertising: AtomicBool,
    powered_on: AtomicBool,
    /// True while at least one central is subscribed to the TX characteristic.
    subscribed: AtomicBool,
    /// The subscribed central's `maximumUpdateValueLength` in bytes, captured in
    /// `didSubscribeToCharacteristic`. Notifications larger than this are never
    /// deliverable (`updateValue` fails without ever signalling ready), so
    /// `drain_outbound` drops oversized frames loudly instead of stalling.
    /// `0` = no subscriber observed yet.
    max_update_len: AtomicUsize,
    inbound: Mutex<Vec<Vec<u8>>>,
    /// Messages waiting to be notified to the subscribed host (guest -> host),
    /// each a queue of that message's chunk frames — grouped so overflow and
    /// undeliverable-frame drops always discard whole messages, never a partial
    /// chunk set the receiver could not reassemble. Drained on the CB queue; a
    /// full CB notify queue re-drains on
    /// `peripheralManagerIsReadyToUpdateSubscribers`.
    outbound: Mutex<VecDeque<VecDeque<Vec<u8>>>>,
    last_error: Mutex<Option<String>>,
    /// The config we want to be advertising, if any.
    pending_config: Mutex<Option<PeripheralConfig>>,
    /// CB objects, created lazily on first `start_advertising`.
    manager: Mutex<Option<AssertSend<Retained<CBPeripheralManager>>>>,
    tx_characteristic: Mutex<Option<AssertSend<Retained<CBMutableCharacteristic>>>>,
    /// Kept alive so the queue backing CB is not dropped while in use.
    queue: Mutex<Option<AssertSend<DispatchRetained<DispatchQueue>>>>,
    /// CBPeripheralManager holds its delegate weakly; keep a strong ref here.
    delegate: Mutex<Option<AssertSend<Retained<NSObject>>>>,
}

impl Shared {
    fn set_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(message.into());
        }
    }
}

pub struct MacosPeripheral {
    shared: Arc<Shared>,
}

impl MacosPeripheral {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                advertising: AtomicBool::new(false),
                powered_on: AtomicBool::new(false),
                subscribed: AtomicBool::new(false),
                max_update_len: AtomicUsize::new(0),
                inbound: Mutex::new(Vec::new()),
                outbound: Mutex::new(VecDeque::new()),
                last_error: Mutex::new(None),
                pending_config: Mutex::new(None),
                manager: Mutex::new(None),
                tx_characteristic: Mutex::new(None),
                queue: Mutex::new(None),
                delegate: Mutex::new(None),
            }),
        }
    }

    /// Create the CB manager (and its queue + delegate) once. Deferred until the
    /// user actually advertises so we do not trigger the Bluetooth permission
    /// prompt at app launch.
    fn ensure_manager(&self) {
        let mut manager_slot = match self.shared.manager.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        if manager_slot.is_some() {
            return;
        }

        let queue = DispatchQueue::new(
            "io.evolve.ble-calculator.peripheral",
            DispatchQueueAttr::SERIAL,
        );
        let delegate = PeripheralDelegate::new(self.shared.clone());
        let delegate_proto: &ProtocolObject<dyn CBPeripheralManagerDelegate> =
            ProtocolObject::from_ref(&*delegate);

        // SAFETY: standard CBPeripheralManager designated initializer.
        let manager: Retained<CBPeripheralManager> = unsafe {
            CBPeripheralManager::initWithDelegate_queue_options(
                CBPeripheralManager::alloc(),
                Some(delegate_proto),
                Some(&*queue),
                None,
            )
        };

        *manager_slot = Some(AssertSend(manager));
        if let Ok(mut queue_slot) = self.shared.queue.lock() {
            *queue_slot = Some(AssertSend(queue));
        }
        if let Ok(mut delegate_slot) = self.shared.delegate.lock() {
            *delegate_slot = Some(AssertSend(Retained::into_super(delegate)));
        }
    }

    /// Marshal `apply_pending` onto the serial CB queue.
    fn dispatch_apply(&self) {
        let queue_guard = match self.shared.queue.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(AssertSend(queue)) = queue_guard.as_ref() else {
            return;
        };
        let shared = self.shared.clone();
        queue.exec_async(move || apply_pending(&shared));
    }

    /// Marshal `drain_outbound` onto the serial CB queue so notify delivery
    /// touches the non-`Send` manager/characteristic only on that queue.
    fn dispatch_drain(&self) {
        let queue_guard = match self.shared.queue.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(AssertSend(queue)) = queue_guard.as_ref() else {
            return;
        };
        let shared = self.shared.clone();
        queue.exec_async(move || drain_outbound(&shared));
    }
}

impl BlePeripheral for MacosPeripheral {
    fn platform(&self) -> &'static str {
        "macos-corebluetooth"
    }

    fn is_supported(&self) -> bool {
        true
    }

    fn start_advertising(&mut self, config: &PeripheralConfig) -> Result<(), String> {
        if let Ok(mut slot) = self.shared.pending_config.lock() {
            *slot = Some(config.clone());
        }
        if let Ok(mut slot) = self.shared.last_error.lock() {
            *slot = None;
        }

        self.ensure_manager();

        // If CoreBluetooth is already powered on, apply immediately; otherwise
        // the delegate's didUpdateState callback applies once it powers on.
        if self.shared.powered_on.load(Ordering::SeqCst) {
            self.dispatch_apply();
        }

        // Advertising is asynchronous (service add -> start -> didStartAdvertising).
        // Wait briefly for the delegate to confirm so we return an accurate result
        // instead of an optimistic Ok (review gap #7). The window is kept short
        // (~200 ms) because this blocks the calling Node worker thread; if neither
        // confirmation nor error arrives in time (e.g. Bluetooth still powering
        // on), treat it as pending Ok — the eventual outcome surfaces on the next
        // poll via `take_last_error` / `is_advertising`.
        for _ in 0..8 {
            if self.shared.advertising.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Take (not peek) so the error is not re-surfaced by the next
            // `take_last_error` poll after we already returned it here.
            if let Ok(mut slot) = self.shared.last_error.lock() {
                if let Some(error) = slot.take() {
                    return Err(error);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        if let Ok(mut slot) = self.shared.pending_config.lock() {
            *slot = None;
        }
        if let Ok(queue_guard) = self.shared.queue.lock() {
            if let Some(AssertSend(queue)) = queue_guard.as_ref() {
                let shared = self.shared.clone();
                queue.exec_async(move || {
                    if let Ok(manager_slot) = shared.manager.lock() {
                        if let Some(AssertSend(manager)) = manager_slot.as_ref() {
                            unsafe {
                                manager.stopAdvertising();
                                manager.removeAllServices();
                            }
                        }
                    }
                    shared.advertising.store(false, Ordering::SeqCst);
                    shared.subscribed.store(false, Ordering::SeqCst);
                    if let Ok(mut outbound) = shared.outbound.lock() {
                        outbound.clear();
                    }
                });
            }
        }
        self.shared.advertising.store(false, Ordering::SeqCst);
        self.shared.subscribed.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn is_advertising(&self) -> bool {
        self.shared.advertising.load(Ordering::SeqCst)
    }

    fn take_inbound(&mut self) -> Vec<Vec<u8>> {
        match self.shared.inbound.lock() {
            Ok(mut inbound) => std::mem::take(&mut *inbound),
            Err(_) => Vec::new(),
        }
    }

    fn notify(&mut self, frames: &[Vec<u8>]) -> Result<usize, String> {
        if frames.is_empty() {
            return Ok(0);
        }
        {
            let mut queue = self
                .shared
                .outbound
                .lock()
                .map_err(|_| "BLE peripheral outbound queue lock was poisoned".to_string())?;
            // One `notify` call = one message's chunk frames = one group.
            queue.push_back(frames.iter().cloned().collect());
            let overflow = queue.len().saturating_sub(MAX_OUTBOUND_MESSAGES);
            if overflow > 0 {
                queue.drain(..overflow);
                self.shared.set_error(format!(
                    "BLE notify queue overflowed; dropped the {overflow} oldest queued message(s). \
                     Events queued before a host subscribes are kept up to {MAX_OUTBOUND_MESSAGES} messages."
                ));
            }
        }
        // Delivery happens on the CB queue; it flushes what it can now and the
        // `isReadyToUpdateSubscribers` callback flushes the rest under backpressure.
        self.dispatch_drain();
        Ok(frames.len())
    }

    fn has_subscriber(&self) -> bool {
        self.shared.subscribed.load(Ordering::SeqCst)
    }

    fn take_last_error(&mut self) -> Option<String> {
        self.shared
            .last_error
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    fn max_notify_frame_len(&self) -> Option<usize> {
        match self.shared.max_update_len.load(Ordering::SeqCst) {
            0 => None,
            len => Some(len),
        }
    }
}

/// Flush queued outbound frames to subscribed centrals over the TX
/// characteristic. Runs on the serial CB queue. Stops when the notify queue is
/// full (`updateValue` returns `false`) and resumes from
/// `peripheralManagerIsReadyToUpdateSubscribers`.
fn drain_outbound(shared: &Arc<Shared>) {
    if !shared.subscribed.load(Ordering::SeqCst) {
        return;
    }
    let manager_guard = match shared.manager.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(AssertSend(manager)) = manager_guard.as_ref() else {
        return;
    };
    let tx_guard = match shared.tx_characteristic.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(AssertSend(tx)) = tx_guard.as_ref() else {
        return;
    };
    let mut outbound = match shared.outbound.lock() {
        Ok(outbound) => outbound,
        Err(_) => return,
    };

    let max_update_len = shared.max_update_len.load(Ordering::SeqCst);
    while let Some(group) = outbound.front_mut() {
        let Some(front) = group.front() else {
            outbound.pop_front();
            continue;
        };
        // A notification larger than the subscribed central's
        // `maximumUpdateValueLength` can never be delivered: `updateValue`
        // keeps returning false and `isReadyToUpdateSubscribers` never fires
        // for it, which would stall every frame behind it forever. Drop the
        // whole message (a partial chunk set can never reassemble anyway) and
        // surface the failure instead.
        if max_update_len > 0 && front.len() > max_update_len {
            shared.set_error(format!(
                "Dropped a queued BLE message with a {}-byte frame; the subscribed host accepts at most {} bytes per notification.",
                front.len(),
                max_update_len
            ));
            outbound.pop_front();
            continue;
        }
        let data = NSData::with_bytes(front);
        // `None` centrals => deliver to every central subscribed to this
        // characteristic. Returns false when the internal transmit queue is
        // full; the remaining frames stay queued for the ready callback.
        let sent = unsafe {
            manager.updateValue_forCharacteristic_onSubscribedCentrals(&data, tx, None)
        };
        if !sent {
            break;
        }
        group.pop_front();
        if group.is_empty() {
            outbound.pop_front();
        }
    }
}

/// Build the GATT service + register it. Runs on the serial CB queue.
/// Advertising is started from `didAddService` once the service is registered.
fn apply_pending(shared: &Arc<Shared>) {
    if !shared.powered_on.load(Ordering::SeqCst) {
        return;
    }
    let config = match shared.pending_config.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(config) => config.clone(),
            None => return,
        },
        Err(_) => return,
    };

    let manager_guard = match shared.manager.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(AssertSend(manager)) = manager_guard.as_ref() else {
        return;
    };

    let service_uuid = unsafe { CBUUID::UUIDWithString(&NSString::from_str(&config.service_uuid)) };
    let rx_uuid =
        unsafe { CBUUID::UUIDWithString(&NSString::from_str(&config.rx_characteristic_uuid)) };
    let tx_uuid =
        unsafe { CBUUID::UUIDWithString(&NSString::from_str(&config.tx_characteristic_uuid)) };

    // RX: host writes calculation events into this characteristic.
    let rx_characteristic = unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &rx_uuid,
            CBCharacteristicProperties::Write | CBCharacteristicProperties::WriteWithoutResponse,
            None,
            CBAttributePermissions::Writeable,
        )
    };
    // TX: guest notifies calculation events to the subscribed host.
    let tx_characteristic = unsafe {
        CBMutableCharacteristic::initWithType_properties_value_permissions(
            CBMutableCharacteristic::alloc(),
            &tx_uuid,
            CBCharacteristicProperties::Notify | CBCharacteristicProperties::Read,
            None,
            CBAttributePermissions::Readable,
        )
    };

    let service = unsafe {
        CBMutableService::initWithType_primary(CBMutableService::alloc(), &service_uuid, true)
    };
    let characteristics: Retained<NSArray<CBCharacteristic>> = NSArray::from_retained_slice(&[
        Retained::into_super(rx_characteristic),
        Retained::into_super(tx_characteristic.clone()),
    ]);
    unsafe { service.setCharacteristics(Some(&characteristics)) };

    if let Ok(mut slot) = shared.tx_characteristic.lock() {
        *slot = Some(AssertSend(tx_characteristic));
    }

    unsafe {
        manager.removeAllServices();
        manager.addService(&service);
    }
}

/// Start advertising the calculator service + local name. Runs on the CB queue.
fn start_advertising_now(shared: &Arc<Shared>) {
    let config = match shared.pending_config.lock() {
        Ok(slot) => match slot.as_ref() {
            Some(config) => config.clone(),
            None => return,
        },
        Err(_) => return,
    };
    let manager_guard = match shared.manager.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(AssertSend(manager)) = manager_guard.as_ref() else {
        return;
    };

    let service_uuid = unsafe { CBUUID::UUIDWithString(&NSString::from_str(&config.service_uuid)) };
    let uuids = NSArray::from_retained_slice(&[service_uuid]);
    let local_name = NSString::from_str(&config.local_name);

    let advertisement = NSMutableDictionary::<NSString, AnyObject>::new();
    // SAFETY: keys are CoreBluetooth advertisement-data constants (NSString),
    // values are the object types CB expects for each key.
    unsafe {
        let uuids_key = ProtocolObject::from_ref(CBAdvertisementDataServiceUUIDsKey);
        let name_key = ProtocolObject::from_ref(CBAdvertisementDataLocalNameKey);
        advertisement.setObject_forKey(uuids.as_ref(), uuids_key);
        advertisement.setObject_forKey(local_name.as_ref(), name_key);
        manager.startAdvertising(Some(&advertisement));
    }
}

// ---------------------------------------------------------------------------
// Delegate
// ---------------------------------------------------------------------------

struct DelegateIvars {
    shared: Arc<Shared>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "EvolveCalcPeripheralDelegate"]
    #[ivars = DelegateIvars]
    struct PeripheralDelegate;

    unsafe impl NSObjectProtocol for PeripheralDelegate {}

    unsafe impl CBPeripheralManagerDelegate for PeripheralDelegate {
        #[unsafe(method(peripheralManagerDidUpdateState:))]
        fn did_update_state(&self, peripheral: &CBPeripheralManager) {
            let shared = &self.ivars().shared;
            let state = unsafe { peripheral.state() };
            let powered_on = state == CBManagerState::PoweredOn;
            shared.powered_on.store(powered_on, Ordering::SeqCst);
            if powered_on {
                apply_pending(shared);
            } else {
                shared.advertising.store(false, Ordering::SeqCst);
                // Distinguish terminal permission/support failures from a
                // transient powered-off/resetting state (review gap #8).
                let message = if state == CBManagerState::Unauthorized {
                    "Bluetooth permission denied. Allow Bluetooth for Evolve Calc in System Settings > Privacy & Security > Bluetooth."
                        .to_string()
                } else if state == CBManagerState::Unsupported {
                    "This device does not support acting as a Bluetooth LE peripheral.".to_string()
                } else if state == CBManagerState::PoweredOff {
                    "Bluetooth is turned off. Turn Bluetooth on to host or join a session.".to_string()
                } else if state == CBManagerState::Resetting {
                    "Bluetooth is resetting; try again in a moment.".to_string()
                } else {
                    format!(
                        "Bluetooth peripheral is not available (CBManagerState = {}).",
                        state.0
                    )
                };
                shared.set_error(message);
            }
        }

        #[unsafe(method(peripheralManager:didAddService:error:))]
        fn did_add_service(
            &self,
            _peripheral: &CBPeripheralManager,
            _service: &CBService,
            error: Option<&NSError>,
        ) {
            let shared = &self.ivars().shared;
            match error {
                Some(error) => shared.set_error(format!(
                    "Failed to add calculator GATT service: {}",
                    error.localizedDescription()
                )),
                None => start_advertising_now(shared),
            }
        }

        #[unsafe(method(peripheralManagerDidStartAdvertising:error:))]
        fn did_start_advertising(&self, _peripheral: &CBPeripheralManager, error: Option<&NSError>) {
            let shared = &self.ivars().shared;
            match error {
                Some(error) => {
                    shared.advertising.store(false, Ordering::SeqCst);
                    shared.set_error(format!(
                        "Failed to start BLE advertising: {}",
                        error.localizedDescription()
                    ));
                }
                None => {
                    shared.advertising.store(true, Ordering::SeqCst);
                    if let Ok(mut slot) = shared.last_error.lock() {
                        *slot = None;
                    }
                }
            }
        }

        #[unsafe(method(peripheralManager:didReceiveWriteRequests:))]
        fn did_receive_write_requests(
            &self,
            peripheral: &CBPeripheralManager,
            requests: &NSArray<CBATTRequest>,
        ) {
            let shared = &self.ivars().shared;
            let mut first: Option<Retained<CBATTRequest>> = None;
            for request in requests {
                if let Some(value) = unsafe { request.value() } {
                    let bytes = value.to_vec();
                    if let Ok(mut inbound) = shared.inbound.lock() {
                        inbound.push(bytes);
                    }
                }
                if first.is_none() {
                    first = Some(request);
                }
            }
            // Responding to the first request in the batch acknowledges them all.
            if let Some(first) = first {
                unsafe { peripheral.respondToRequest_withResult(&first, CBATTError::Success) };
            }
        }

        #[unsafe(method(peripheralManager:central:didSubscribeToCharacteristic:))]
        fn did_subscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            // A host subscribed to the TX characteristic: the guest -> host link
            // is live. Capture how large each notification may be for this
            // central (Apple: read `maximumUpdateValueLength` at runtime, never
            // hardcode), then flush anything already queued.
            let shared = &self.ivars().shared;
            let max_update_len = unsafe { central.maximumUpdateValueLength() };
            shared.max_update_len.store(max_update_len, Ordering::SeqCst);
            shared.subscribed.store(true, Ordering::SeqCst);
            drain_outbound(shared);
        }

        #[unsafe(method(peripheralManager:central:didUnsubscribeFromCharacteristic:))]
        fn did_unsubscribe(
            &self,
            _peripheral: &CBPeripheralManager,
            _central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            // The host dropped its subscription; hold queued frames until a host
            // subscribes again rather than dropping them.
            self.ivars().shared.subscribed.store(false, Ordering::SeqCst);
        }

        #[unsafe(method(peripheralManagerIsReadyToUpdateSubscribers:))]
        fn is_ready_to_update(&self, _peripheral: &CBPeripheralManager) {
            // CoreBluetooth's notify queue drained; resume sending under backpressure.
            drain_outbound(&self.ivars().shared);
        }
    }
);

impl PeripheralDelegate {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests never call `start_advertising`, so no CBPeripheralManager (and
    // no Bluetooth permission prompt) is created: `dispatch_drain` is a no-op
    // while the CB queue is unset, leaving frames observable in `outbound`.

    #[test]
    fn notify_overflow_drops_oldest_messages_and_surfaces_error() {
        let mut peripheral = MacosPeripheral::new();
        // Each notify call queues one message (of two chunk frames here).
        for message in 0..MAX_OUTBOUND_MESSAGES + 10 {
            let frames = vec![
                message.to_le_bytes().to_vec(),
                message.to_le_bytes().to_vec(),
            ];
            peripheral.notify(&frames).expect("notify should queue frames");
        }

        let queue = peripheral.shared.outbound.lock().expect("outbound lock");
        assert_eq!(queue.len(), MAX_OUTBOUND_MESSAGES);
        // Whole oldest messages were dropped; the front group is message #10,
        // still holding both of its frames.
        let front = queue.front().expect("front message");
        assert_eq!(front.len(), 2);
        assert_eq!(front.front(), Some(&10usize.to_le_bytes().to_vec()));
        drop(queue);

        let error = peripheral.take_last_error().expect("overflow surfaces an error");
        assert!(error.contains("oldest queued message"), "unexpected error: {error}");
        // Take semantics: the error is reported once, not on every poll.
        assert_eq!(peripheral.take_last_error(), None);
    }

    #[test]
    fn notify_below_cap_keeps_every_message_and_reports_no_error() {
        let mut peripheral = MacosPeripheral::new();
        assert_eq!(peripheral.notify(&[vec![1u8], vec![2u8]]), Ok(2));
        assert_eq!(peripheral.notify(&[vec![3u8]]), Ok(1));

        let queue = peripheral.shared.outbound.lock().expect("outbound lock");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.front().map(|group| group.len()), Some(2));
        drop(queue);
        assert_eq!(peripheral.take_last_error(), None);
    }
}

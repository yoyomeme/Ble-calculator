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

use std::sync::atomic::{AtomicBool, Ordering};
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
use objc2_foundation::{NSArray, NSError, NSMutableDictionary, NSString};

use super::{BlePeripheral, PeripheralConfig};

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
    inbound: Mutex<Vec<Vec<u8>>>,
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
                inbound: Mutex::new(Vec::new()),
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

        // Advertising is asynchronous; report the current known error (if any).
        match self.shared.last_error.lock() {
            Ok(slot) => match slot.as_ref() {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            },
            Err(_) => Ok(()),
        }
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
                });
            }
        }
        self.shared.advertising.store(false, Ordering::SeqCst);
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
                shared.set_error(format!(
                    "Bluetooth peripheral is not available (CBManagerState = {}).",
                    state.0
                ));
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
            _central: &CBCentral,
            _characteristic: &CBCharacteristic,
        ) {
            // A host subscribed to the TX characteristic; outbound notify
            // delivery is handled by the calculator's outbox drain.
        }
    }
);

impl PeripheralDelegate {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

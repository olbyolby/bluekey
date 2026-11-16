pub(super) mod data;

use std::sync::{Arc, Weak};

use crate::bluetooth::leds::Led;
use crate::bluetooth::{DeviceMap, Register, ReturnEventListener};

use super::{Target, ServerError, Never};

use super::hid::{self, Notifiers, NotifyInbox, Protocol};
use super::hid::characteristics::callback;

use bluer::{AdapterEvent, Address};
use bluer::{Adapter, gatt::local::{Characteristic, CharacteristicNotifier, CharacteristicNotifyFun}};
use futures::{FutureExt, StreamExt, pin_mut};
use tokio::sync::{RwLock, mpsc, broadcast};
use log::{debug, info, warn};
use tokio::task::JoinHandle;




#[derive(Clone, Copy, Debug)]
enum KeyboardEvent {
    PressKey(Target, u8),
    ReleaseKey(Target, u8)
}
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum KeyboardReturnEvent {
    Register(Address),
    LedOn(Address, super::leds::Led),
    LedOff(Address, super::leds::Led),
    Suspend(Address),
    Wake(Address)
}
impl From<Register> for KeyboardReturnEvent {
    fn from(value: Register) -> Self {
        Self::Register(value.0)   
    }
}



#[derive(Clone, Copy, Debug)]
pub struct KeyboardServerDied;
#[derive(Clone, Copy, Debug)]
pub enum KeyboardTrySendError {
    ServerDied,
    QueueFull
}

pub struct Keyboard {
    channel: mpsc::Sender<KeyboardEvent>,
    _returns: broadcast::Receiver<KeyboardReturnEvent>, 
    pub(super) returns_sender: broadcast::Sender<KeyboardReturnEvent>,
    pub(super) state: Weak<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>,
    handle: JoinHandle<Result<Never, ServerError >>,
    // Keyboard specific GATT characteristics, taken once by bluetooth::service::serve
    characteristics: std::sync::Mutex<Option<Vec<Characteristic>>>
}
impl Keyboard {
    pub fn new(adapter: Arc<Adapter>) -> Keyboard {
        let (keyboard_sender, keyboard_receiver) = mpsc::channel(16);
        let (return_sender, return_receiver) = broadcast::channel(16);

        // Create the keyboard server
        let server_state = Arc::new(RwLock::new(DeviceMap::new(return_sender.clone())));

        // The keyboard's characteristics are served as part of the single, shared HID service(see bluetooth::service)
        let (boot_input_handle, boot_input_control) = hid::notify_channel();
        let (report_input_handle, report_input_control) = hid::notify_channel();
        let characteristics = keyboard_characteristics(server_state.clone(), return_sender.clone(), boot_input_handle, report_input_handle);

        let handle = tokio::spawn(keyboard_server(keyboard_receiver, adapter, server_state.clone(), boot_input_control, report_input_control).inspect(|error| {
            warn!("{}", error.as_ref().unwrap_err())
        }));

        Keyboard {
            channel: keyboard_sender,
            _returns: return_receiver,
            returns_sender: return_sender,
            state: Arc::downgrade(&server_state),
            handle,
            characteristics: std::sync::Mutex::new(Some(characteristics))
        }
    }

    // Hands the keyboard's GATT characteristics to the shared HID service, can only be taken once
    pub(super) fn take_characteristics(&self) -> Vec<Characteristic> {
        self.characteristics.lock().unwrap().take().unwrap_or_default()
    }



    pub async fn press(&self, target: Target, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::PressKey(target, keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    #[allow(dead_code)]
    pub fn try_press(&self, target: Target, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::PressKey(target, keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }
    pub async fn release(&self, target: Target, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::ReleaseKey(target, keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    #[allow(dead_code)]
    pub fn try_release(&self, target: Target, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::ReleaseKey(target, keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }

    pub fn listen(&self) -> ReturnEventListener<KeyboardReturnEvent> {
        ReturnEventListener { receiver: self.returns_sender.subscribe() }
    }

    pub fn devices(&self,) -> Result<DevicesView, KeyboardServerDied> {
        Ok(DevicesView { state: self.state.upgrade().ok_or(KeyboardServerDied)? })

    }

    pub async fn close(&mut self) -> Result<Never, ServerError> {
        (&mut self.handle).await.unwrap() // Propagate panic
    }


}
impl Drop for Keyboard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// Until RwLockReadGuard::map() is stabalized, we have to do this annoying mess.
// See: https://github.com/rust-lang/rust/issues/117108
pub struct DevicesView {
    state: Arc<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>
}
impl DevicesView {
    #[allow(dead_code)]
    pub async fn collect<T: FromIterator<Address>>(&self) -> T {
        let lock = self.state.read().await;
        T::from_iter(lock.devices.keys().cloned())
    }
    pub async fn for_each<F: Fn(Address) -> ()>(&self, map: F) {
        let lock = self.state.read().await;
        for address in lock.devices.keys().cloned() {
            map(address);
        }
    }
    pub async fn get<F: FnOnce(Option<&KeyboardState>) -> T, T>(&self, address: Address, callback: F) -> T {
        let lock = self.state.read().await;
        callback(lock.get_device(address))
    }
}

pub struct KeyboardState {
    keys: [u8; 6],
    modifiers: u8,
    leds: u8,

    pub(super) protocol: Protocol
}
impl Default for KeyboardState {
    fn default() -> Self {
        DEFAULT_STATE
    }
}
impl KeyboardState {
    // Boot and report protocol share the same 8 byte layout for the keyboard
    async fn send_report(&self, notifiers: &mut Notifiers) {
        let mut report = [0; 8];
        report[0] = self.modifiers;
        report[2..].clone_from_slice(&self.keys);

        debug!("Keyboard report {:?}", report);
        notifiers.send(self.protocol, &report).await;
    }

    pub fn led_state(&self) -> impl Iterator<Item=(Led, bool)> {
        (1..=5).map(|i| (i-1, Led::from_usb_id(i).unwrap())).map(|(offset, led)| {
            (led, (1<<offset) & self.leds != 0)
        })
    }
}

const DEFAULT_STATE: KeyboardState = KeyboardState {
    keys: [0,0,0,0,0,0],
    modifiers: 0,
    leds: 0,

    protocol: Protocol::Report
};


// Keyboard specific characteristics of the HID service. The shared ones(protocol mode, HID information,
// control point and report map) are created by bluetooth::service, since there can only be one of each per service.
fn keyboard_characteristics(state: Arc<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>, return_sender: broadcast::Sender<KeyboardReturnEvent>, boot_input_handle: CharacteristicNotifyFun, report_input_handle: CharacteristicNotifyFun) -> Vec<Characteristic> {
    vec![
        hid::characteristics::boot_keyboard_input(
            callback!(|request| state {
                let state = state.read().await;
                let state = state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE);

                debug!("Read boot keyboard by {}", request.device_address);
                let mut data = vec![state.modifiers, 0x00];
                data.extend_from_slice(&state.keys);
                Ok(data)
            }),
            boot_input_handle
        ),
        hid::characteristics::boot_keyboard_output(
            callback!(|request| {
                debug!("Boot keyboard output by {}", request.device_address);
                Ok(vec![0])
            }),
            callback!(|value, request| state {
                let mut state = state.write().await;
                let state = state.acquire_device(request.device_address).await;

                debug!("Value {:?} from {}", value, request.device_address);
                state.leds = value[0];
                Ok(())
            })
        ),
        hid::characteristics::input_report(
            callback!(|request| state {
                let state = state.read().await;
                let state = state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE);

                debug!("Report read by {}", request.device_address);
                let mut data = vec![state.modifiers, 0x00];
                data.extend_from_slice(&state.keys);
                Ok(data)
            }),
            report_input_handle,
            hid::KEYBOARD_REPORT_ID
        ),
        hid::characteristics::output_report(
            callback!(|request| state {
                debug!("LEDs read by {:?}", request.device_address);
                Ok(vec![state.read().await.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).leds])
            }),
            callback!(|data, request| state,return_sender {
                use super::leds::Led;

                let mut state = state.write().await;
                let device = state.acquire_device(request.device_address).await;
                debug!("LEDs writen with {:?} by {:?}", data, request.device_address);
                let now_on = data[0] & !device.leds;
                let now_off  = device.leds & !data[0];
                device.leds = data[0];
                drop(state);

                debug!("off: {:b}, on: {:b}", now_off, now_on);
                for (offset, led) in (1..=5).map(|i| (i-1, Led::from_usb_id(i).unwrap())) {
                    if (1<<offset) & now_on != 0 {
                        debug!("{:?} went on", led);
                        return_sender.send(KeyboardReturnEvent::LedOn(request.device_address, led)).unwrap();
                    } else if (1<<offset )& now_off != 0 {
                        debug!("{:?} went off", led);
                        return_sender.send(KeyboardReturnEvent::LedOff(request.device_address, led)).unwrap();
                    }
                }

                Ok(())
            }),
            hid::KEYBOARD_REPORT_ID
        )
    ]
}

async fn keyboard_server(mut receiver: mpsc::Receiver<KeyboardEvent>, adapter: Arc<Adapter>, state: Arc<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>, mut boot_input_control: NotifyInbox, mut report_input_control: NotifyInbox) -> Result<Never, ServerError > {
    let disconnect_events = adapter.events().await?.filter_map(async |event|{
        match event {
            AdapterEvent::DeviceRemoved(address) => Some(address),
            _ => None
        }
    });

    // Combine the streams
    enum Event {
        Keyboard(KeyboardEvent),
        BootReportNotify(CharacteristicNotifier),
        ReportNotify(CharacteristicNotifier),
        DeviceDisconnect(Address)
    }

    pin_mut!(disconnect_events);
    let mut events = async move || {
        tokio::select! {
            event = receiver.recv() => Ok::<Event, ServerError>(Event::Keyboard(event.expect("Keyboard event stream dropped without aborting server"))),
            notifier = boot_input_control.next() => Ok(Event::BootReportNotify(notifier)),
            notifier = report_input_control.next() => Ok(Event::ReportNotify(notifier)),
            event = disconnect_events.next() => Ok(Event::DeviceDisconnect(event.ok_or(ServerError::AdapterLost)?))
        }
    };

    let mut notifiers = Notifiers::new("keyboard");

    info!("Successfully created keyboard server");
    loop {
        let event = events().await?;
        let mut state = state.write().await;
        match event {
            Event::Keyboard(KeyboardEvent::PressKey(target, keycode)) => {
                for device in state.get_targets(target) {
                    // Check if the key is a modifier 
                    if (0xE0..=0xE7).contains(&keycode) {
                        let index = keycode - 0xE0;
                        device.modifiers |= 0x1<<index; // Set the modifier's bit field
                        device.send_report(&mut notifiers).await;
                    } else if !device.keys.contains(&keycode) {
                        if let Some(empty) = device.keys.iter().position(|k| *k == 0) {
                            device.keys[empty] = keycode;
                        };
                        device.send_report(&mut notifiers).await;
                    }
                }

            },
            Event::Keyboard(KeyboardEvent::ReleaseKey(target, keycode)) => {
                for device in state.get_targets(target) {
                    // Check if the key is a modifier 
                    if (0xE0..=0xE7).contains(&keycode) {
                        let index = keycode - 0xE0;
                        device.modifiers &= !(0x1<<index); // Clear the modifier's bit field
                        device.send_report(&mut notifiers).await;
                    } else if let Some(key) = device.keys.iter().position(|k| *k==keycode) {
                        device.keys[key] = 0;
                        device.send_report(&mut notifiers).await;
                    }
                }
            },
            Event::BootReportNotify(notifier) => notifiers.subscribed(Protocol::Boot, notifier),
            Event::ReportNotify(notifier) => notifiers.subscribed(Protocol::Report, notifier),
            Event::DeviceDisconnect(address) => {
                state.disconnect_device(address).await;
            },
        }
    }



}



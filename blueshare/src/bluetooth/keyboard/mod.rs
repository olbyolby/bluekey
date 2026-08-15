mod data;

use std::sync::{Arc, Weak};

use crate::bluetooth::leds::Led;
use crate::bluetooth::{DeviceMap, Register, ReturnEventListener};

use super::{Target, ServerError, Never};

use super::hid::{self, Protocol};
use super::hid::characteristics::callback;

use bluer::{AdapterEvent, Address};
use bluer::{Adapter, adv::Advertisement, gatt::{CharacteristicWriter, local::{Application, CharacteristicControlEvent, ReqError, Service, characteristic_control}}};
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
    returns_sender: broadcast::Sender<KeyboardReturnEvent>,
    state: Weak<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>,
    handle: JoinHandle<Result<Never, ServerError >>
}
impl Keyboard {
    pub fn new(adapter: Arc<Adapter>) -> Keyboard {
        let (keyboard_sender, keyboard_receiver) = mpsc::channel(16);
        let (return_sender, return_receiver) = broadcast::channel(16);

        // Create the keyboard server
        let server_state = Arc::new(RwLock::new(DeviceMap::new(return_sender.clone())));
        let handle = tokio::spawn(keyboard_server(keyboard_receiver, return_sender.clone(), adapter, server_state.clone()).inspect(|error| {
            warn!("{}", error.as_ref().unwrap_err())
        }));

        Keyboard { channel: keyboard_sender, _returns: return_receiver, returns_sender: return_sender, state: Arc::downgrade(&server_state), handle }
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

    protocol: Protocol,
    boot: Option<CharacteristicWriter>,
    report: Option<CharacteristicWriter>
}
impl Default for KeyboardState {
    fn default() -> Self {
        DEFAULT_STATE
    }
}
impl KeyboardState {
    async fn send_report(&self) {
        let report = {
            let mut report = [0; 8];
            report[0] = self.modifiers;
            report[2..].clone_from_slice(&self.keys);
            
            report
        };

        let reporter = match self.protocol {
            Protocol::Boot => &self.boot,
            Protocol::Report => &self.report
        };
        if let Some(reporter) = reporter {
            reporter.send(&report).await.unwrap();
        }

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

    protocol: Protocol::Report,
    boot: None, 
    report: None
};


async fn keyboard_server(mut receiver: mpsc::Receiver<KeyboardEvent>, return_sender: broadcast::Sender<KeyboardReturnEvent>, adapter: Arc<Adapter>, state: Arc<RwLock<DeviceMap<KeyboardState, KeyboardReturnEvent>>>) -> Result<Never, ServerError > {
    let disconnect_events = adapter.events().await?.filter_map(async |event|{
        match event {
            AdapterEvent::DeviceRemoved(address) => Some(address),
            _ => None
        }
    });
    
    // Start advertising the keyboard functionality
    let _advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::definitions::SERVICE].into(),
       discoverable: Some(true),
       appearance: Some(0x03C1),

       ..Default::default()
    }).await?;

    // Create the GATT service
    let (mut boot_input_control, boot_input_handle) = characteristic_control();
    let (mut report_input_control, report_input_handle) = characteristic_control();
    let _application_handle = adapter.serve_gatt_application(Application {
        services: vec![Service {
            uuid: hid::definitions::SERVICE,
            primary: true,
            characteristics: vec![
                hid::characteristics::protocol_mode(
                    callback!(|request| state {
                        let state = state.read().await;

                        debug!("Read protocol by {}", request.device_address);
                        Ok(vec![state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).protocol.into()])
                    }),
                    callback!(|value, request| state {
                        debug!("Write protocal by {} with {:?}", request.device_address, value);
                        let protocol = match value.get(0) {
                            Some(0) => Some(Protocol::Boot),
                            Some(1) => Some(Protocol::Report),
                            _ => None
                        };
                        if let Some(protocol) = protocol {
                            state.write().await.acquire_device(request.device_address).await.protocol = protocol;
                        };
                        Ok(())
                    })
                ),
                hid::characteristics::information(hid::HID_INFORMATION),
                hid::characteristics::control_point(callback!(|value, request| return_sender {
                    debug!("Write control point by {} with {:?}", request.device_address, value);
                    return_sender.send(match value[0] {
                        0 => Ok(KeyboardReturnEvent::Suspend(request.device_address)),
                        1 => Ok(KeyboardReturnEvent::Wake(request.device_address)),
                        _ => Err(ReqError::Failed)
                    }?).unwrap();
                    Ok(())
                })),
                hid::characteristics::report_map(data::REPORT_DESCRIPTOR),
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
                    report_input_handle
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
                )
            ],
            ..Default::default()
        }],
        ..Default::default()
    }).await?;

    // Combine the streams
    enum Event {
        Keyboard(KeyboardEvent),
        BootReportNotify(CharacteristicWriter),
        ReportNotify(CharacteristicWriter),
        DeviceDisconnect(Address)
    }
    
    pin_mut!(disconnect_events);
    let mut events = async move || {
        tokio::select! {
            event = receiver.recv() => Ok::<Event, ServerError>(Event::Keyboard(event.expect("Keyboard event stream dropped without aborting server"))),
            event = boot_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Ok(Event::BootReportNotify(writer)),
                _ => unreachable!()
            },
            event = report_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Ok(Event::ReportNotify(writer)),
                _ => unreachable!()
            },
            event = disconnect_events.next() => Ok(Event::DeviceDisconnect(event.ok_or(ServerError::AdapterLost)?))
        }
    };

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
                        device.send_report().await;
                    } else if !device.keys.contains(&keycode) {
                        if let Some(empty) = device.keys.iter().position(|k| *k == 0) {
                            device.keys[empty] = keycode;
                        };
                        device.send_report().await;
                    }
                }
                
            },
            Event::Keyboard(KeyboardEvent::ReleaseKey(target, keycode)) => {
                for device in state.get_targets(target) {
                    // Check if the key is a modifier 
                    if (0xE0..=0xE7).contains(&keycode) {
                        let index = keycode - 0xE0;
                        device.modifiers &= !(0x1<<index); // Clear the modifier's bit field
                        device.send_report().await;
                    } else if let Some(key) = device.keys.iter().position(|k| *k==keycode) {
                        device.keys[key] = 0;
                        device.send_report().await;
                    }
                }
            },
            Event::BootReportNotify(writer) => {
                let address = writer.device_address();
                state.acquire_device(address).await.boot = Some(writer)
            },
            Event::ReportNotify(writer) => {
                let address = writer.device_address();
                state.acquire_device(address).await.report = Some(writer)
            }
            Event::DeviceDisconnect(address) => {
                state.disconnect_device(address).await;
            },
        }
    }



}



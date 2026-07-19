mod data;

use std::sync::Arc;

use super::hid::{self, Protocol};
use super::hid::characteristics::callback;

use bluer::{Adapter, adv::Advertisement, gatt::{CharacteristicWriter, local::{Application, CharacteristicControlEvent, ReqError, Service, characteristic_control}}};
use futures::StreamExt;
use tokio::sync::{RwLock, mpsc};
use log::debug;

#[derive(Clone, Copy, Debug)]
enum KeyboardEvent {
    PressKey(u8),
    ReleaseKey(u8)
}
#[derive(Clone, Copy, Debug)]
pub enum KeyboardReturnEvent {
    LedOn(super::leds::Led),
    LedOff(super::leds::Led),
    Suspend,
    Wake
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
    returns: mpsc::Receiver<KeyboardReturnEvent>
}
impl Keyboard {
    pub async fn press(&self, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::PressKey(keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    #[allow(dead_code)]
    pub fn try_press(&self, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::PressKey(keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }
    pub async fn release(&self, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::ReleaseKey(keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    #[allow(dead_code)]
    pub fn try_release(&self, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::ReleaseKey(keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }

    pub async fn next_event(&mut self) -> Result<KeyboardReturnEvent, KeyboardServerDied> {
        match self.returns.recv().await {
            Some(event) => Ok(event),
            None => Err(KeyboardServerDied)
        }
    }
}

pub async fn start_keyboard(adapter: Arc<Adapter>) -> Keyboard {
    let (keyboard_sender, keyboard_receiver) = mpsc::channel(16);
    let (return_sender, return_receiver) = mpsc::channel(16);

    // Create the keyboard server
    tokio::spawn(keyboard_server(keyboard_receiver, return_sender, adapter));

    Keyboard { channel: keyboard_sender, returns: return_receiver }
}




struct KeyboardState {
    keys: [u8; 6],
    modifiers: u8,
    leds: u8,

    boot_input: Vec<CharacteristicWriter>,
    report_input: Vec<CharacteristicWriter>,
    protocol: Protocol
}
impl Default for KeyboardState {
    fn default() -> Self {
        KeyboardState {
            keys: [0,0,0,0,0,0],
            modifiers: 0,
            leds: 0,

            boot_input: Vec::new(),
            report_input: Vec::new(),
            protocol: Protocol::Report
        }
    }
}

async fn keyboard_server(mut receiver: mpsc::Receiver<KeyboardEvent>, return_sender: mpsc::Sender<KeyboardReturnEvent>, adapter: Arc<Adapter>) {
    let state = Arc::new(RwLock::new(<KeyboardState as Default>::default()));

    // Start advertising the keyboard functionality
    let advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::definitions::SERVICE].into(),
       discoverable: Some(true),
       appearance: Some(0x03C1),

       ..Default::default()
    }).await.expect("Error creating advertisement");

    // Create the GATT service
    let (mut boot_input_control, boot_input_handle) = characteristic_control();
    let (mut report_input_control, report_input_handle) = characteristic_control();
    let application_handle = adapter.serve_gatt_application(Application {
        services: vec![Service {
            uuid: hid::definitions::SERVICE,
            primary: true,
            characteristics: vec![
                hid::characteristics::protocol_mode(
                    callback!(|_request| state {
                        debug!("Read protocol by {}", _request.device_address);
                        Ok(vec![state.read().await.protocol.into()])
                    }),
                    callback!(|value, _request| state {
                        debug!("Write protocal by {} with {:?}", _request.device_address, value);
                        let protocol = match value.get(0) {
                            Some(0) => Some(Protocol::Boot),
                            Some(1) => Some(Protocol::Report),
                            _ => None
                        };
                        if let Some(protocol) = protocol {
                            state.write().await.protocol = protocol;
                        };
                        Ok(())
                    })
                ),
                hid::characteristics::information(hid::HID_INFORMATION),
                hid::characteristics::control_point(callback!(|value, _request| return_sender {
                    debug!("Write control point by {} with {:?}", _request.device_address, value);
                    return_sender.send(match value[0] {
                        0 => Ok(KeyboardReturnEvent::Suspend),
                        1 => Ok(KeyboardReturnEvent::Wake),
                        _ => Err(ReqError::Failed)
                    }?).await.unwrap();
                    Ok(())
                })),
                hid::characteristics::report_map(data::REPORT_DESCRIPTOR),
                hid::characteristics::boot_keyboard_input(
                    callback!(|_request| state {
                        let state = state.read().await;

                        debug!("Read boot keyboard by {}", _request.device_address);
                        let mut data = vec![state.modifiers, 0x00];
                        data.extend_from_slice(&state.keys);
                        Ok(data)
                    }),
                    boot_input_handle
                ),
                hid::characteristics::boot_keyboard_output(
                    callback!(|_request| {
                        debug!("Boot keyboard output by {}", _request.device_address);
                        Ok(vec![0])
                    }),
                    callback!(|value, read| {
                        debug!("Value {:?} from {}", value, read.device_address);
                        Ok(())
                    })
                ),
                hid::characteristics::input_report(
                    callback!(|_request| state {
                        let state = state.read().await;

                        debug!("Report read by {}", _request.device_address);
                        let mut data = vec![state.modifiers, 0x00];
                        data.extend_from_slice(&state.keys);
                        Ok(data)
                    }),
                    report_input_handle
                ),
                hid::characteristics::output_report(
                    callback!(|request| state {
                        debug!("LEDs read by {:?}", request.device_address);
                        Ok(vec![state.read().await.leds])
                    }),
                    callback!(|data, request| state,return_sender {
                        use super::leds::Led;

                        let mut state = state.write().await;
                        debug!("LEDs writen with {:?} by {:?}", data, request.device_address);
                        let now_on = data[0] & !state.leds;
                        let now_off  = state.leds & !data[0];
                        state.leds = data[0];
                        drop(state);

                        debug!("off: {:b}, on: {:b}", now_off, now_on);
                        for (id, led) in (1..=5).map(|i| (i, Led::try_from(i).unwrap())) {
                            if (1<<id) & now_on != 0 {
                                debug!("{:?} went on", led);
                                return_sender.send(KeyboardReturnEvent::LedOn(led)).await.unwrap();
                            } else if (1<<id )& now_off != 0 {
                                debug!("{:?} went off", led);
                                return_sender.send(KeyboardReturnEvent::LedOff(led)).await.unwrap();
                            }
                        }
                                                
                        Ok(())
                    }), 
                )
            ],
            ..Default::default()
        }],
        ..Default::default()
    }).await.expect("Failed to start GATT server");

    // Combine the streams
    enum Event {
        Keyboard(KeyboardEvent),
        BootReportNotify(CharacteristicWriter),
        ReportNotify(CharacteristicWriter)
    }
    let mut events = async move || {
        tokio::select! {
            event = receiver.recv() => event.map(|event| Event::Keyboard(event)),
            event = boot_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::BootReportNotify(writer)),
                _ => panic!("Invalid")
            },
            event = report_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::ReportNotify(writer)),
                _ => panic!("Invalid")
            }
        }
    };

    while let Some(event) = events().await {
        let mut state = state.write().await;
        match event {
            Event::Keyboard(KeyboardEvent::PressKey(keycode)) => {
                // Check if the key is a modifier 
                if (0xE0..=0xE7).contains(&keycode) {
                    let index = keycode - 0xE0;
                    state.modifiers |= 0x1<<index;

                    send_update(&mut state).await;
                } else if !state.keys.contains(&keycode) {
                    if let Some(empty) = state.keys.iter().position(|k| *k == 0) {
                        state.keys[empty] = keycode;

                        send_update(&mut state).await;
                    };
                }
            },
            Event::Keyboard(KeyboardEvent::ReleaseKey(keycode)) => {
                if (0xE0..=0xE7).contains(&keycode) {
                    let index = keycode - 0xE0;
                    state.modifiers &= !(0x1<<index);

                    send_update(&mut state).await;
                } else if let Some(key) = state.keys.iter().position(|k| *k==keycode) {
                    state.keys[key] = 0;
                    send_update(&mut state).await;
                }
            },
            Event::BootReportNotify(writer) => state.boot_input.push(writer),
            Event::ReportNotify(writer) => state.report_input.push(writer)
        }
    }

    drop(advertisement_handle);
    drop(application_handle);
}

async fn send_update(state: &mut KeyboardState) {
    let mut event = vec![state.modifiers, 0x00];
    event.extend_from_slice(&state.keys);

    debug!("Sending event {:?} on protocol {:?}", event, state.protocol);
    let listeners = match state.protocol {
        Protocol::Boot => &mut state.boot_input,
        Protocol::Report => &mut state.report_input
    };

    listeners.retain(|l| !l.is_closed().unwrap_or(true));
    for listener in listeners.iter_mut() {
        if let Err(err) =  listener.send(&event).await {
            debug!("ERRR {:?} on {:?}", err, state.protocol);
        }
    };
}


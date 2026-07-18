mod data;

use std::sync::Arc;

use super::hid;
use bluer::{Adapter, adv::Advertisement, gatt::local::{Application, CharacteristicNotifier, Service}};
use tokio::sync::{RwLock, mpsc};

#[derive(Clone, Copy, Debug)]
enum KeyboardEvent {
    PressKey(u8),
    ReleaseKey(u8)
}

#[derive(Clone, Copy, Debug)]
pub struct KeyboardServerDied;
#[derive(Clone, Copy, Debug)]
pub enum KeyboardTrySendError {
    ServerDied,
    QueueFull
}

pub struct Keyboard {
    channel: mpsc::Sender<KeyboardEvent>
}
impl Keyboard {
    #[allow(dead_code)]
    pub async fn press(&self, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::PressKey(keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    pub fn try_press(&self, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::PressKey(keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }
    #[allow(dead_code)]
    pub async fn release(&self, keycode: u8) -> Result<(), KeyboardServerDied> {
        match self.channel.send(KeyboardEvent::ReleaseKey(keycode)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(KeyboardServerDied)
        }
    }
    pub fn try_release(&self, keycode: u8) -> Result<(), KeyboardTrySendError> {
        match self.channel.try_send(KeyboardEvent::ReleaseKey(keycode)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(KeyboardTrySendError::QueueFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(KeyboardTrySendError::ServerDied)
        }
    }
}

pub async fn start_keyboard(adapter: Adapter) -> bluer::Result<Keyboard> {
    let (sender, receiver) = mpsc::channel(16);

    // Create the keyboard server
    tokio::spawn(keyboard_server(receiver, adapter));

    Ok(Keyboard { channel: sender })
}

#[derive(Clone, Copy, Debug)]
enum Protocol {
    Boot,
    Report
}
impl Into<u8> for Protocol {
    fn into(self) -> u8 {
        match self {
            Self::Boot => 0,
            Self::Report => 1
        }
    }
}

// The amount of cloning bullshit I had to do here is going to drive me to extremism
// Blur's callbacks want some werid cursed call signature and that gets repetitive to write.
// First a clone needs to be made so it can be moved into the callback without consuming the state for everyone,
// Then another copy as to be made into the async section of the callback because the async bit may outlive the function.
macro_rules! callback {
    (|$($arg:ident),*| $state:ident $code:block) => {
        {
            let $state = $state.clone();
            Box::new(move |$($arg),*| {
                let $state = $state.clone();
                Box::pin(async move $code)
            })
        }
    };
    (|$($arg:ident),*| $code:block) => {
        Box::new(move |$($arg),*| {
            Box::pin(async move $code)
        })
    }
}

struct KeyboardState {
    keys: [u8; 6],
    modifiers: u8,

    boot_input: Vec<CharacteristicNotifier>,
    report_input: Vec<CharacteristicNotifier>,
    protocol: Protocol
}
impl Default for KeyboardState {
    fn default() -> Self {
        KeyboardState {
            keys: [0,0,0,0,0,0],
            modifiers: 0,

            boot_input: Vec::new(),
            report_input: Vec::new(),
            protocol: Protocol::Report
        }
    }
}

async fn keyboard_server(mut receiver: mpsc::Receiver<KeyboardEvent>, adapter: Adapter) {
    let state = Arc::new(RwLock::new(<KeyboardState as Default>::default()));

    // Start advertising the keyboard functionality

    let advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::KEYBOARD].into(),
       discoverable: Some(true),
       appearance: Some(0x03C1),

       ..Default::default()
    }).await.expect("Error creating advertisement");

    // Create the GATT service
    let application_handle = adapter.serve_gatt_application(Application {
        services: vec![Service {
            uuid: hid::KEYBOARD,
            primary: true,
            characteristics: vec![
                characteristics::protocol_mode(
                    callback!(|_request| state {
                        println!("Read protocol by {}", _request.device_address);
                        Ok(vec![state.read().await.protocol.into()])
                    }),
                    callback!(|value, _request| state {
                        println!("Write protocal by {} with {:?}", _request.device_address, value);
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
                characteristics::information(data::HID_INFORMATION),
                characteristics::control_point(callback!(|value, _request| {
                    println!("Write control point by {} with {:?}", _request.device_address, value);
                    Ok(())
                })),
                characteristics::report_map(data::REPORT_DESCRIPTOR),
                characteristics::boot_keyboard_input(
                    callback!(|_request| state {
                        let state = state.read().await;

                        println!("Read boot keyboard by {}", _request.device_address);
                        let mut data = vec![state.modifiers, 0x00];
                        data.extend_from_slice(&state.keys);
                        Ok(data)
                    }),
                    callback!(|notifier| state {
                        state.write().await.boot_input.push(notifier);
                    })
                ),
                characteristics::boot_keyboard_output(
                    callback!(|_request| {
                        println!("Boot keyboard output by {}", _request.device_address);
                        Ok(vec![0])
                    }),
                    callback!(|value, read| {
                        println!("Value {:?} from {}", value, read.device_address);
                        Ok(())
                    })
                ),
                characteristics::report(
                    callback!(|_request| state {
                        let state = state.read().await;

                        println!("Report read by {}", _request.device_address);
                        let mut data = vec![state.modifiers, 0x00];
                        data.extend_from_slice(&state.keys);
                        Ok(data)
                    }),
                    callback!(|notifier| state {
                        println!("Report notiifer");
                        state.write().await.report_input.push(notifier)
                    })
                )
            ],
            ..Default::default()
        }],
        ..Default::default()
    }).await.expect("Failed to start GATT server");

    while let Some(event) = receiver.recv().await {
        let mut state = state.write().await;
        match event {
            KeyboardEvent::PressKey(keycode) => {
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
            KeyboardEvent::ReleaseKey(keycode) => {
                if (0xE0..=0xE7).contains(&keycode) {
                    let index = keycode - 0xE0;
                    state.modifiers &= !(0x1<<index);

                    send_update(&mut state).await;
                } else if let Some(key) = state.keys.iter().position(|k| *k==keycode) {
                    state.keys[key] = 0;
                    send_update(&mut state).await;
                }
            }
        }
    }

    drop(advertisement_handle);
    drop(application_handle);
}

async fn send_update(state: &mut KeyboardState) {
    let mut event = vec![state.modifiers, 0x00];
    event.extend_from_slice(&state.keys);

    println!("Sending event {:?} on protocol {:?}", event, state.protocol);
    let listeners = match state.protocol {
        Protocol::Boot => &mut state.boot_input,
        Protocol::Report => &mut state.report_input
    };

    listeners.retain(|l| !l.is_stopped());
    for listener in listeners.iter_mut() {
        if let Err(err) =  listener.notify(event.clone()).await {
            println!("ERRR {:?} on {:?}", err, state.protocol);
        }
    };
}


mod characteristics {
    use bluer::gatt::local::{Characteristic, CharacteristicNotify, CharacteristicNotifyFun, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicReadFun, CharacteristicWrite, CharacteristicWriteFun, CharacteristicWriteMethod, Descriptor, DescriptorRead};

    use super::hid;
    
    pub(super) fn protocol_mode(read: CharacteristicReadFun, write: CharacteristicWriteFun) -> Characteristic {
        
        Characteristic {
            uuid: hid::characteristics::PROTOCOL_MODE,
            read: Some(CharacteristicRead {
                read: true,
                fun: read,
                ..Default::default()
            }),
            write: Some(CharacteristicWrite {
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(write),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub(super) fn information(descriptor: &'static [u8]) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::INFORMATION,
            read: Some(CharacteristicRead {
                read: true,
                fun: Box::new(move |_request| Box::pin(async move {
                    Ok(descriptor.into())
                })),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    pub(super) fn control_point(write: CharacteristicWriteFun) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::CONTROL_POINT,
            write: Some(CharacteristicWrite { 
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(write),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    pub(super) fn report_map(report_descripter: &'static [u8]) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::REPORT_MAP,
            read: Some(CharacteristicRead {
                read: true,
                fun: Box::new(move |_rquest| Box::pin(async move {
                    Ok(report_descripter.into())
                })),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub(super) fn boot_keyboard_input(reader: CharacteristicReadFun, notifier: CharacteristicNotifyFun) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::boot::keyboard::INPUT,
            read: Some(CharacteristicRead {
                read: true,
                fun: reader,
                ..Default::default()
            }),
            notify: Some(CharacteristicNotify {
                notify: true,
                method: CharacteristicNotifyMethod::Fun(notifier),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    pub(super) fn boot_keyboard_output(reader: CharacteristicReadFun, writer: CharacteristicWriteFun) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::boot::keyboard::OUTPUT,
            read: Some(CharacteristicRead {
                read: true,
                fun: reader,
                ..Default::default()
            }),
            write: Some(CharacteristicWrite {
                write: true,
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(writer),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    pub(super) fn report(read: CharacteristicReadFun, notifier: CharacteristicNotifyFun) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::REPORT,
            read: Some(CharacteristicRead {
                read: true,
                fun: read,
                ..Default::default()
            }),
            notify: Some(CharacteristicNotify {
                notify: true,
                method: CharacteristicNotifyMethod::Fun(notifier),
                ..Default::default()
            }),
            descriptors: vec![Descriptor {
                uuid: hid::descriptors::REPORT_REFERENCE,
                read: Some(DescriptorRead {
                    read: true,
                    fun: Box::new(|request| {
                        Box::pin(async move {
                            println!("Descirptor for HID_REPORT read by {}", request.device_address);
                            Ok([0x00, 0x01].into())
                        })
                    }),

                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
}

mod data;

use std::sync::{Arc, atomic::{AtomicBool, AtomicU8}};

use crate::hid;
use bluer::{Adapter, adv::Advertisement, gatt::{CharacteristicWriter, local::Application}};
use tokio::sync::{RwLock, mpsc};


enum KeyboardEvent {

}
struct Keyboard {
    channel: mpsc::Sender<KeyboardEvent>
}

async fn start_keyboard(adapter: Adapter) -> bluer::Result<Keyboard> {
    let (sender, receiver) = mpsc::channel(16);

    // Create the advertisement
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


struct KeyboardState {
    keys: [u8; 6],
    modifiers: u8,

    boot_input: Vec<CharacteristicWriter>,
    report_input: Vec<CharacteristicWriter>,
    protocol: Protocol
}
impl Default for KeyboardState {
    fn default() -> Self {
        KeyboardState {
            keys: [0,0,0,0,0,0],
            modifiers: 0,

            boot_input: Vec::new(),
            report_input: Vec::new(),
            protocol: Protocol::Boot
        }
    }
}

async fn keyboard_server(receiver: mpsc::Receiver<KeyboardEvent>, adapter: Adapter) {
    let state = Arc::new(RwLock::new(<KeyboardState as Default>::default()));

    // Start advertising the keyboard functionality
    let advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::KEYBOARD].into(),
       discoverable: Some(true),

       ..Default::default()
    });

    // Create the GATT service

}


mod characteristics {
    use std::sync::{Arc, atomic::Ordering};

    use bluer::gatt::local::{Characteristic, CharacteristicNotify, CharacteristicNotifyMethod, CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, characteristic_control};
use tokio::sync::RwLock;

    use crate::{hid, keyboard::Protocol};
    use super::KeyboardState;
    type State = Arc<RwLock<KeyboardState>>;
    
    pub(super) fn protocol_mode(state: State) -> Characteristic {
        // The amount of cloning bullshit I had to do here is going to drive me to extremism
        Characteristic {
            uuid: hid::characteristics::PROTOCOL_MODE,
            read: Some(CharacteristicRead {
                read: true,
                fun: {
                    let state = state.clone();
                    Box::new(move |_| {
                        let state = state.clone();
                        Box::pin(async move {
                            Ok(vec![state.read().await.protocol.into()])
                        })
                    }
                )},
                ..Default::default()
            }),
            write: Some(CharacteristicWrite {
                write_without_response: true,
                method: {
                    let state = state.clone();
                    CharacteristicWriteMethod::Fun(Box::new(move |value, _request| {
                        let state = state.clone();
                        Box::pin(async move {
                            let protocol = match value.get(0) {
                                Some(0) => Some(Protocol::Boot),
                                Some(1) => Some(Protocol::Report),
                                _ => None
                            };
                            if let Some(protocol) = protocol {
                                state.write().await.protocol = protocol;
                            }
                            Ok(())
                        })
                    }))
                },
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
    pub(super) fn control_point() -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::CONTROL_POINT,
            write: Some(CharacteristicWrite { 
                write_without_response: true,
                method: CharacteristicWriteMethod::Fun(Box::new(move |value, request| Box::pin(async move {
                    // Nothing to be done here. \0/
                    Ok(())
                }))),
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
                fun: Box::new(move |request| Box::pin(async move {
                    println!("REPORT_MAP read by {} from {}", request.adapter_name, request.device_address);
                    Ok(report_descripter.into())
                })),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    pub(super) fn boot_keyboard_input(state: State) -> Characteristic {
        Characteristic {
            uuid: hid::characteristics::boot::keyboard::INPUT,
            read: Some(CharacteristicRead {
                read: true,
                fun: {
                    let state = state.clone();
                    Box::new(move |request| {
                        let state = state.clone();
                        Box::pin(async move {
                            let state = state.read().await;
                            let mut v = vec![state.modifiers, 0x00];
                            v.extend_from_slice(&state.keys);
                            Ok(v)
                        })
                    })
                },
                ..Default::default()
            }),
            notify: Some(CharacteristicNotify {
                notify: true,
                method: {
                    let state = state.clone();
                    CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
                        let state = state.clone();
                        Box::pin(async move {
                            let state = state.write().await;
                            state.boot_input.append(notifer);
                        })
                    }))
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

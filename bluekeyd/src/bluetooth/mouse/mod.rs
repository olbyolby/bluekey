use std::sync::{Arc, Weak};

use bluer::Address;
use bluer::gatt::CharacteristicWriter;
use bluer::gatt::local::{Application, CharacteristicControlEvent, ReqError, Service};
use bluer::{Adapter, adv::Advertisement, gatt::local::characteristic_control};
use futures::StreamExt;
use log::{debug, info};
use tokio::sync::{RwLock, broadcast, mpsc};

use crate::bluetooth::{DeviceMap, Register, ReturnEventListener};

use super::hid::{self, Protocol};
use super::hid::characteristics::callback;
use super::Target;

mod data;

#[derive(Clone, Copy, Debug)]
pub struct InvalidButton;
#[derive(Clone, Copy, Debug)]
pub struct Button {
    pub id: u16
}
impl Button {
    pub fn from_id(id: u16) -> Result<Self, InvalidButton> {
        match id {
            0 => Err(InvalidButton),
            _ => Ok(Button { id })
        }
    }
    pub fn into_u16(&self) -> u16 {
        self.id
    }
}
impl Into<u16> for Button {
    fn into(self) -> u16 {
        self.into_u16()
    }
}
impl TryFrom<u16> for Button {
    type Error = InvalidButton;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::from_id(value)
    }
}


#[derive(Clone, Copy, Debug)]
pub struct MouseServerDied;

#[derive(Clone, Copy, Debug)]
pub enum TryMouseError {
    MouseServerDied,
    QueueFull
}
#[derive(Clone, Copy, Debug)]
pub enum MouseReturnError {
    ServerDied,
    Lagged(u64)
}

#[derive(Clone, Copy, Debug)]
enum MouseEvent {
    ButtonPress(Target, Button),
    ButtonRelease(Target, Button),
    Movement(Target, i8, i8, i8),
}
#[derive(Clone, Copy, Debug)]
pub enum MouseReturnEvent {
    Register(Address),
    Wake(Address),
    Suspend(Address)
}
impl From<Register> for MouseReturnEvent {
    fn from(value: Register) -> Self {
        MouseReturnEvent::Register(value.0)
    }
}

pub struct Mouse {
    channel: mpsc::Sender<MouseEvent>,
    returns: broadcast::Receiver<MouseReturnEvent>,
    returns_sender: broadcast::Sender<MouseReturnEvent>,
    handle: tokio::task::JoinHandle<()>,
    state: Weak<RwLock<MouseServer>>
}
#[allow(dead_code)]
impl Mouse {
    pub fn new(adapter: Arc<Adapter>) -> Self {
        let (mouse_sender, mouse_receiver) = mpsc::channel(16);
        let (return_sender, return_receiver) = broadcast::channel(16);

        // Create the mouse server
        let state = Arc::new(RwLock::new(DeviceMap::new(return_sender.clone())));
        let handle = tokio::spawn(mouse_server(mouse_receiver, return_sender.clone(), adapter, state.clone()));

        Mouse {
            channel: mouse_sender,
            returns: return_receiver,
            returns_sender: return_sender,
            state: Arc::downgrade(&state),
            handle
        }
    }

    pub async fn press(&self, target: Target, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonPress(target, button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_press(&self, target: Target, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonPress(target, button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn release(&self, target: Target, button: Button) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::ButtonRelease(target, button)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }

    pub fn try_release(&self, target: Target, button: Button) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::ButtonRelease(target, button)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }
    pub async fn moved(&self, target: Target, x: i8, y: i8, scroll: i8) -> Result<(), MouseServerDied> {
        match self.channel.send(MouseEvent::Movement(target, x, y, scroll)).await {
            Ok(_) => Ok(()),
            Err(mpsc::error::SendError(_)) => Err(MouseServerDied)
        }
    }
    
    pub fn try_moved(&self, target: Target, x: i8, y: i8, scroll: i8) -> Result<(), TryMouseError> {
        match self.channel.try_send(MouseEvent::Movement(target, x, y, scroll)) {
            Ok(_) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(TryMouseError::MouseServerDied),
            Err(mpsc::error::TrySendError::Full(_)) => Err(TryMouseError::QueueFull),
        }
    }

    pub fn listen(&self) -> ReturnEventListener<MouseReturnEvent> {
        ReturnEventListener { receiver: self.returns_sender.subscribe() }
    }

    pub fn cancel(&self) {
        self.handle.abort();
    }
}




type MouseServer = DeviceMap<IndividualMouse, MouseReturnEvent>;
struct IndividualMouse {
    protocol: Protocol,
    buttons: u8,

    report: Option<CharacteristicWriter>,
    boot: Option<CharacteristicWriter>
}
impl IndividualMouse {
    async fn send_report(&mut self, movement: Option<(i8, i8)>, wheel: Option<i8>) {
        let (mx, my) = movement.unwrap_or((0, 0));
        let wheel = wheel.unwrap_or(0);
        
        let (notifier, report) = match self.protocol {
            Protocol::Boot => (&mut self.boot, &[
                self.buttons, // Current state of buttons
                mx as u8, // Preserve bit pattern
                my as u8,
            ] as &[u8]),
            Protocol::Report => (&mut self.report, &[
                self.buttons, // Current state of buttons
                mx as u8, // Preserve bit pattern
                my as u8,
                wheel as u8
            ] as &[u8])
        };
        //println!("Sending report on protocol {:?} to {:?}", self.protocol, notifier.as_ref().map(|v| v.device_address()));

        if let Some(notifier) = notifier {
            if let Err(err) = notifier.send(report).await {
                debug!("ERRR {:?} on {:?}", err, self.protocol);
            }
        }
    }
}
impl Default for IndividualMouse {
    fn default() -> Self {
        IndividualMouse {
            protocol: Protocol::Report,
            buttons: 0,

            report: None,
            boot: None
        }   
    }
}

const DEFAULT_STATE: IndividualMouse = IndividualMouse {
    protocol: Protocol::Report,
    buttons: 0,

    boot: None,
    report: None
};

async fn mouse_server(mut receiever: mpsc::Receiver<MouseEvent>, return_sender: broadcast::Sender<MouseReturnEvent>, adapter: Arc<Adapter>, state: Arc<RwLock<MouseServer>>) {

    // Start advertising the keyboard functionality
    let advertisement_handle = adapter.advertise(Advertisement {
       advertisement_type: bluer::adv::Type::Peripheral,
       service_uuids: [hid::definitions::SERVICE].into(),
       discoverable: Some(true),
       appearance: Some(0x03c2),

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
                    callback!(|request| state {
                        debug!("Read protocol by {}", request.device_address);
                        Ok(vec![state.read().await.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).protocol.into()])
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
                hid::characteristics::information(super::hid::HID_INFORMATION),
                hid::characteristics::control_point(callback!(|value, request| return_sender {
                    debug!("Write control point by {} with {:?}", request.device_address, value);
                    return_sender.send(match value[0] {
                        0 => Ok(MouseReturnEvent::Suspend(request.device_address)),
                        1 => Ok(MouseReturnEvent::Wake(request.device_address)),
                        _ => Err(ReqError::Failed)
                    }?).unwrap();
                    Ok(())
                })),
                hid::characteristics::report_map(data::REPORT_DESCRIPTOR),
                hid::characteristics::boot_mouse_input(
                    callback!(|request| state {
                        let state = state.read().await;

                        debug!("Boot report  read by {}", request.device_address);
                        Ok(vec![state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).buttons, 0, 0])
                    }),
                    boot_input_handle
                ),
                hid::characteristics::input_report(
                    callback!(|request| state {
                        let state = state.read().await;

                        debug!("Report read by {}", request.device_address);
                        Ok(vec![state.get_device(request.device_address).unwrap_or(&DEFAULT_STATE).buttons, 0, 0])
                    }),
                    report_input_handle
                )
            ],
            ..Default::default()
        }],
        ..Default::default()
    }).await.expect("Failed to start GATT server");
    drop(adapter);


    enum Event {
        BootReportNotify(CharacteristicWriter),
        ReportNotify(CharacteristicWriter),
        MouseEvent(MouseEvent)
    }
    let mut events = async move || {
        tokio::select! {
            event = boot_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::BootReportNotify(writer)),
                _ => panic!("Invalid")
            },
            event = report_input_control.next() => match event {
                Some(CharacteristicControlEvent::Notify(writer)) => Some(Event::ReportNotify(writer)),
                _ => panic!("Invalid")
            },
            event = receiever.recv() => match event {
                Some(event) => Some(Event::MouseEvent(event)),
                None => None
            }
        }
    };

    info!("Successfully created mouse server");
    while let Some(event) = events().await {
        let mut state = state.write().await;
        match event {
            Event::BootReportNotify(writer) => {
                let state = state.acquire_device(writer.device_address()).await;
                state.boot = Some(writer)
            },
            Event::ReportNotify(writer) => {
                let state = state.acquire_device(writer.device_address()).await;
                state.report = Some(writer)
            },
            Event::MouseEvent(event) => match event {
                MouseEvent::ButtonPress(target, button) => {
                    
                    for device in state.get_targets(target) {
                        // Subtract one from button ID to account for button 1 being at bit 0
                        if (1..=3).contains(&button.into_u16()) && device.buttons & (1<<(button.into_u16()-1)) == 0 {
                            device.buttons |= 1<<(button.into_u16()-1);
                            device.send_report(None, None).await;
                        }
                    }
                },
                MouseEvent::ButtonRelease(target, button) => {
                    for device in state.get_targets(target) {
                        // Subtract one from button ID to account for button 1 being at bit 0
                        if (1..=3).contains(&button.into_u16()) && device.buttons & (1<<(button.into_u16()-1)) != 0 {
                            device.buttons &= !(1<<(button.into_u16()-1));
                            device.send_report(None, None).await;
                        }
                    }
                    
                },
                MouseEvent::Movement(target, x, y, scroll) => {
                    for device in state.get_targets(target) {
                        device.send_report(Some((x, y)), Some(scroll)).await
                    }
                },
            }
        }
    }

    drop(advertisement_handle);
    drop(application_handle);
}
